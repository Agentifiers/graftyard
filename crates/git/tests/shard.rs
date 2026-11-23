//! `shards_for`, the start cap, the region check, the inactivity renewal, and the image map.

use std::collections::BTreeMap;

use graftyard_git::*;
use graftyard_types::{DbId, NetworkId, Region, SiteId, Timestamp};

const H: u64 = 3_600_000;

fn net(n: i64) -> NetworkId {
    NetworkId(DbId(n))
}

/// A Tri group's record on `sites`, home first.
fn tri(sites: &[SiteId]) -> graftyard_types::PlacementRecord {
    graftyard_types::PlacementRecord {
        name: graftyard_types::PlacementName::Tri,
        sites: sites.iter().copied().collect(),
        home: sites[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
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
fn container_day_covers_about_10_50_100_percent_of_shard_objects_each_step_covering_the_last() {
    let all: Vec<String> = (0..2_000).map(|i| format!("{:064x}", i * 7919 + 13)).collect();
    let on = |site: u8, step: Option<u8>| {
        all.iter().filter(|o| image_for(SiteId(site), o, step) == ImageSlot::Git).collect::<Vec<_>>()
    };
    assert!(on(1, None).is_empty(), "before the day, and after its rollback: git_prev");
    let (ten, half, full) = (on(1, Some(0)), on(1, Some(1)), on(1, Some(2)));
    assert!((140..=260).contains(&ten.len()), "{} of 2,000 at 10%", ten.len());
    assert!((900..=1_100).contains(&half.len()), "{} of 2,000 at 50%", half.len());
    assert_eq!(full.len(), 2_000);
    assert!(ten.iter().all(|o| half.contains(o)), "50% covers every shard 10% did");
    // Another site's day picks other objects: the share is drawn per site.
    assert_ne!(on(2, Some(0)), ten);
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
        let composed = futures_executor::block_on(chain_ready(follows, true, left, ready, |w| {
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

/// After its own catch-up, which shard takes the composed-miss chain's step before any miss asks
/// (`steps_after_catch_up`, `chain-install`): the builder builds, and every shard on a chain site installs, so the chain
/// site that takes the misses when the home shard goes already holds a chain. B0-34 mt12's case: network 1048605, home
/// a, chain sites a and c. Site c served none of the 46 misses before the kill, so on main's rule (only the builder)
/// its shards held no chain at the kill, and its first miss waited 13.8 s for the install. A shard on b, which holds no
/// chain, still waits for a miss to ask.
#[test]
fn a_shard_on_a_chain_site_installs_the_chain_after_its_catch_up() {
    use graftyard_git::compose::steps_after_catch_up;
    use graftyard_types::{DbId, NetworkId, SiteId};
    let net = NetworkId(DbId(1048605));
    let rec = tri(&[SiteId(0), SiteId(1), SiteId(2)]);
    let chain = graftyard_git::chain_buckets(net, &rec);
    assert_eq!(chain, [SiteId(0), SiteId(2)], "mt12's chain buckets: a and c");
    // (site, builder) -> steps after a catch-up
    let steps = |site: SiteId, builder: bool| steps_after_catch_up(net, &rec, site, builder);
    assert!(steps(SiteId(0), true), "the builder steps");
    assert!(steps(SiteId(2), false), "a shard on chain site c installs before the kill sends it misses");
    assert!(steps(SiteId(0), false), "another shard on the home site, a chain site, installs too");
    assert!(!steps(SiteId(1), false), "a shard on b, which holds no chain, waits for a miss");
    // Another network whose chain sites are a and b: c, not a chain site there, does not step.
    let other = (1..2_000_000)
        .map(|n| NetworkId(DbId(n)))
        .find(|n| graftyard_git::chain_buckets(*n, &rec) == [SiteId(0), SiteId(1)]);
    let other = other.expect("a network whose chain sites are a and b");
    assert!(!steps_after_catch_up(other, &rec, SiteId(2), false), "c holds no chain for that network");
    assert!(steps_after_catch_up(other, &rec, SiteId(1), false), "b does");
}

/// A composable miss on a site whose bucket holds no chain (`chain_ready` with `chain_site` false, a site that is not
/// one of `chain_buckets`) waits for no chain step. As the last site it asks its container at once, which composes
/// from a chain it already holds or serves stock; forwarded, it answers `warming` at once so the edge asks the next
/// site, unless a chain is already synced there. B0-34 mt11's case: site c, last in kill's order and not a warm site,
/// waited `COMPOSE_WAIT`'s whole 20 s (`ready_ms` 20000) for each of 5 misses, then served stock or composed at 22 to
/// 32 s; 4 of them took kill's every-clone share from 92 % to 88.2 % (`tests/am1/results/b034-mt11.md`). On main's
/// rule the last site waits all that is left of its deadline, which fails here.
#[test]
fn a_miss_on_a_site_that_holds_no_chain_waits_for_no_chain_step() {
    use graftyard_git::compose::chain_ready;
    use graftyard_types::{DbId, NetworkId, SiteId};
    use std::cell::Cell;
    use std::task::Poll;
    use std::time::Duration;
    let s = Duration::from_secs;
    // mt11's monorepo network on graft-m1's three sites, home a: the chain sites are a and b, not c.
    let net = NetworkId(DbId(1048604));
    let chain = graftyard_git::chain_buckets(net, &tri(&[SiteId(0), SiteId(1), SiteId(2)]));
    let chain_site = |s: SiteId| chain.contains(&s);
    let c = SiteId(2);
    assert_eq!(chain, [SiteId(0), SiteId(1)], "mt11's chain buckets: a and b");
    // (asks its container, the wait the shard slept for)
    let run = |follows: bool, on: SiteId, left: Duration, ready_at: Option<Duration>| {
        let slept = Cell::new(None::<Duration>);
        let ready = std::future::poll_fn(|_| match (slept.get(), ready_at) {
            (Some(w), Some(r)) if r <= w => Poll::Ready(()),
            _ => Poll::Pending,
        });
        let go = futures_executor::block_on(chain_ready(follows, chain_site(on), left, ready, |w| {
            slept.set(Some(w));
            std::future::poll_fn(
                move |_| if ready_at.is_some_and(|r| r <= w) { Poll::Pending } else { Poll::Ready(()) },
            )
        }));
        (go, slept.get().expect("the shard sleeps for its wait"))
    };
    // mt11's case: site c last (wait=60, no next), no chain ever. It asks its container at once, not after 20 s or 60 s.
    assert_eq!(run(false, c, s(60), None), (true, Duration::ZERO));
    // Forwarded (next=1) from site c: warming at once, not after the 2 s bound.
    assert_eq!(run(true, c, s(15), None), (false, Duration::ZERO));
    // A chain already synced on site c still answers there, last or forwarded.
    assert_eq!(run(false, c, s(60), Some(Duration::ZERO)), (true, Duration::ZERO));
    assert_eq!(run(true, c, s(15), Some(Duration::ZERO)), (true, Duration::ZERO));
    // The chain sites keep `chain-wait`'s rule: a forwarded miss waits the bound, the last site its whole deadline.
    for on in chain.iter().copied() {
        assert_eq!(run(true, on, s(15), None), (false, s(2)), "chain site {on:?} forwarded");
        assert_eq!(run(false, on, s(60), None), (false, s(60)), "chain site {on:?} last");
        assert_eq!(run(false, on, s(60), Some(s(14))), (true, s(60)), "chain site {on:?} composes a chain 14 s away");
    }
}

/// A clone-cache hit's lookup runs beside its read gate (`beside_gate`, `hit-gate`), on a virtual clock: the hit's
/// time to an answer is the longer of the two, not their sum, and a gate that does not allow keeps nothing the lookup
/// found. B0-34 mt11's slow manifest hits: gate 0.47 s (a cold access read) then manifest read and chain checks 0.8 s
/// gave 1.27 s one after the other; beside each other, 0.8 s. On main's rule (gate, then lookup) this fails.
#[test]
fn a_hits_lookup_runs_beside_its_gate_and_a_refused_gate_keeps_nothing() {
    use graftyard_git::compose::beside_gate;
    use std::cell::Cell;
    use std::rc::Rc;
    // A virtual clock in ms: each future ends at its own time, and only the future whose end is the earliest pending
    // one moves the clock there, so a future still pending never moves time past one that ends sooner.
    #[derive(Clone)]
    struct Clock(Rc<Cell<u64>>, Rc<std::cell::RefCell<Vec<u64>>>);
    impl Clock {
        fn at(&self, end: u64) -> impl std::future::Future<Output = u64> + use<> {
            let (now, ends) = (self.0.clone(), self.1.clone());
            ends.borrow_mut().push(end);
            std::future::poll_fn(move |cx| {
                if now.get() >= end {
                    let mut e = ends.borrow_mut();
                    if let Some(i) = e.iter().position(|x| *x == end) {
                        e.remove(i);
                    }
                    return std::task::Poll::Ready(end);
                }
                if ends.borrow().iter().copied().filter(|e| *e >= now.get()).min() == Some(end) {
                    now.set(end);
                }
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            })
        }
    }
    let run = |gate_ms: u64, allow: bool, lookup_ms: u64| {
        let clock = Clock(Rc::new(Cell::new(0)), Rc::default());
        let looked = Rc::new(Cell::new(false));
        let gate = {
            let g = clock.at(gate_ms);
            async move {
                g.await;
                allow
            }
        };
        let lookup = {
            let (l, looked) = (clock.at(lookup_ms), looked.clone());
            async move {
                l.await;
                looked.set(true);
                "hit"
            }
        };
        let (g, hit) = futures_executor::block_on(beside_gate(gate, lookup, |g| *g));
        (g, hit, clock.0.get(), looked.get())
    };
    // mt11's slow hit: the answer is ready at the lookup's 800 ms, not at 470 + 800.
    assert_eq!(run(470, true, 800), (true, Some("hit"), 800, true));
    // A lookup quicker than the gate waits for the gate's allow before it counts.
    assert_eq!(run(900, true, 300), (true, Some("hit"), 900, true));
    // A gate that refuses first drops the lookup unfinished: nothing kept, and no wait for it.
    assert_eq!(run(200, false, 800), (false, None, 200, false));
    // A gate that refuses after the lookup ended drops its value.
    assert_eq!(run(900, false, 300), (false, None, 900, true));
}

/// A manifest answer's line read from chunks (`LineSplit`, `hit-gate`): the line is held across chunks, the tail starts
/// in the chunk the line ended in, and a line past `MANIFEST_MAX` is refused without holding more. Main read the whole
/// answer, line and tail, into one buffer and copied the tail once more, so an 8 MiB tail held 16.8 MB in the edgebed.
#[test]
fn a_manifest_line_is_split_from_its_tail_across_chunks() {
    use graftyard_git::compose::{LineSplit, LineStep, MANIFEST_MAX};
    let mut s = LineSplit::default();
    assert_eq!(s.push(b"{\"v\":1,"), LineStep::More);
    assert_eq!(s.push(b"\"x\":2}\nTAIL"), LineStep::Line(b"{\"v\":1,\"x\":2}\n".to_vec(), 7));
    // A line that ends a chunk leaves the tail to the next one.
    let mut s = LineSplit::default();
    assert_eq!(s.push(b"{}\n"), LineStep::Line(b"{}\n".to_vec(), 3));
    // A line in one chunk with its tail.
    let mut s = LineSplit::default();
    assert_eq!(s.push(b"{}\n\nmore\n"), LineStep::Line(b"{}\n".to_vec(), 3));
    // No newline within MANIFEST_MAX: refused, across chunks or in one.
    let mut s = LineSplit::default();
    assert_eq!(s.push(&vec![b'a'; MANIFEST_MAX - 1]), LineStep::More);
    assert_eq!(s.push(b"a"), LineStep::TooLong);
    let mut s = LineSplit::default();
    let mut long = vec![b'a'; MANIFEST_MAX];
    long.push(b'\n');
    assert_eq!(s.push(&long), LineStep::TooLong);
    // One byte under the limit still reads.
    let mut s = LineSplit::default();
    let mut ok = vec![b'a'; MANIFEST_MAX - 1];
    ok.push(b'\n');
    assert_eq!(s.push(&ok), LineStep::Line(ok.clone(), MANIFEST_MAX));
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

/// A virtual clock for the edge's composed-answer rules: `run` moves it to the next timer whenever every future waits,
/// and panics when every future waits on something no timer will end (a hang).
#[derive(Default)]
struct Clock {
    now: std::cell::Cell<u64>,
    timers: std::cell::RefCell<Vec<(u64, std::task::Waker)>>,
    in_flight: std::cell::Cell<usize>,
    most: std::cell::Cell<usize>,
    started: std::cell::Cell<usize>,
}

impl Clock {
    async fn sleep(&self, ms: u64) {
        let until = self.now.get() + ms;
        std::future::poll_fn(|cx| {
            if self.now.get() >= until {
                return std::task::Poll::Ready(());
            }
            self.timers.borrow_mut().push((until, cx.waker().clone()));
            std::task::Poll::Pending
        })
        .await
    }
    async fn open(&self, i: usize, ms: u64, missing: bool) -> Result<Option<usize>, String> {
        self.started.set(self.started.get() + 1);
        self.in_flight.set(self.in_flight.get() + 1);
        self.most.set(self.most.get().max(self.in_flight.get()));
        self.sleep(ms).await;
        self.in_flight.set(self.in_flight.get() - 1);
        Ok((!missing).then_some(i))
    }
    fn run<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            let mut t = self.timers.borrow_mut();
            let next = t.iter().map(|(u, _)| *u).min().expect("every future waits and no timer is set: a hang");
            self.now.set(next);
            let due: Vec<_> = t.extract_if(.., |(u, _)| *u <= next).collect();
            drop(t);
            for (_, w) in due {
                w.wake();
            }
        }
    }
}

/// The edge's composed answer opens its chain packs `CHAIN_OPEN_AT_ONCE` at a time, not one after another, and hands
/// them on in the manifest's order (`decisions/DECISIONS.md`, "`chain-open`"). On main `open_chain` waited for each
/// pack's first chunk before it asked for the next: a base and 19 parts were 20 R2 round trips before any header,
/// and clone-cache hits went from p50 0.6 s (m1b34, mt4) to 1.6 and 2.0 s once edge-compose served them (mt5, mt6).
/// Here each open takes its own time on a virtual clock, the later ones faster, so a run that waits on each in turn,
/// or hands them on as they finish, or opens them all at once, gives a different answer.
#[test]
fn a_composed_answer_opens_its_chain_packs_at_once_and_keeps_their_order() {
    use graftyard_git::compose::{CHAIN_OPEN_AT_ONCE, open_in_order};
    assert_eq!(CHAIN_OPEN_AT_ONCE, 4);
    // A base and 19 parts; open i takes 200 - 5i ms, so later packs open first.
    let ms = |i: usize| 200 - 5 * i as u64;
    let c = Clock::default();
    let got = c.run(open_in_order((0..20).map(|i| c.open(i, ms(i), false)))).unwrap();
    assert_eq!(got, Some((0..20).collect::<Vec<_>>()), "the packs in the manifest's order");
    let serial: u64 = (0..20).map(ms).sum();
    assert!(c.now.get() <= serial / 3, "{} ms to open 20 packs, {serial} ms one after another", c.now.get());
    assert_eq!(c.most.get(), CHAIN_OPEN_AT_ONCE, "at most CHAIN_OPEN_AT_ONCE R2 gets wait for headers at once");
    // A missing pack: no answer, and the opens past the window are never started.
    let c = Clock::default();
    let got = c.run(open_in_order((0..20).map(|i| c.open(i, ms(i), i == 2)))).unwrap();
    assert_eq!(got, None);
    assert!(c.started.get() < 20, "{} opens started after a missing pack", c.started.get());
    // One pack, as a fresh chain's base alone: its own time.
    let c = Clock::default();
    assert_eq!(c.run(open_in_order([c.open(0, 120, false)])).unwrap(), Some(vec![0]));
    assert_eq!(c.now.get(), 120);
}

/// A composed answer holds at most one chain pack's R2 body at a time, so a chain of any length streams, and its chain
/// checks refuse within `CHAIN_CHECK_WITHIN` instead of waiting (`decisions/DECISIONS.md`, "`chain-hold`"). One Workers
/// invocation that held eight R2 bodies open got no ninth `get` (graft-m1, 2026-11-20), and main's `open_chain` held
/// every pack's body from its check to its turn in the answer, so B0-34 mt9's clones of a 9-pack chain or more hung 120
/// s with no byte. Here R2 is that model: a body is held from its `get` until `send` lets it go, and a `get` past eight
/// held never answers, which `Clock::run` reports as a hang.
#[test]
fn a_composed_answer_streams_a_long_chain_holding_one_body_and_refuses_a_slow_check_in_time() {
    use graftyard_git::compose::{CHAIN_CHECK_WITHIN, ChainCheck, check_in_time, stream_in_turn};
    use std::cell::{Cell, RefCell};
    const HELD_MAX: usize = 8;
    let c = Clock::default();
    let (held, most, sent) = (Cell::new(0usize), Cell::new(0usize), RefCell::new(Vec::new()));
    // A body `get`: 30 ms, then held; with HELD_MAX held already it waits on nothing, as R2 did.
    let get = async |i: usize| -> Result<usize, String> {
        if held.get() >= HELD_MAX {
            std::future::pending::<()>().await;
        }
        c.sleep(30).await;
        held.set(held.get() + 1);
        most.set(most.get().max(held.get()));
        Ok(i)
    };
    // Sending a body takes 50 ms, then lets it go.
    let send = async |i: usize| -> Result<(), String> {
        c.sleep(50).await;
        sent.borrow_mut().push(i);
        held.set(held.get() - 1);
        Ok(())
    };
    // A base and 19 parts, as a chain is before its fold.
    c.run(stream_in_turn(0..20, get, send)).unwrap();
    assert_eq!(*sent.borrow(), (0..20).collect::<Vec<_>>(), "every pack, in the manifest's order");
    assert_eq!(most.get(), 1, "one body held at a time");
    assert_eq!(held.get(), 0);
    // A failed send ends the answer there: no later pack is opened.
    let opened = Cell::new(0);
    let r = c.run(stream_in_turn(
        0..20,
        async |i: usize| {
            opened.set(opened.get() + 1);
            Ok::<_, String>(i)
        },
        async |i: usize| if i == 3 { Err(format!("pack {i} cut")) } else { Ok(()) },
    ));
    assert_eq!((r, opened.get()), (Err("pack 3 cut".into()), 4));

    // The checks: within the clone line, with room for the first bytes after them.
    assert!(CHAIN_CHECK_WITHIN <= std::time::Duration::from_secs(4), "{CHAIN_CHECK_WITHIN:?}");
    let bound = CHAIN_CHECK_WITHIN.as_millis() as u64;
    // 20 checks at 100 ms, four at a time: checked, in order, well inside the bound.
    let c = Clock::default();
    let got = c.run(check_in_time([0], |_| (0..20).map(|i| c.open(i, 100, false)), |d| c.sleep(d.as_millis() as u64)));
    assert_eq!(got, Ok(ChainCheck::Checked((0, (0..20).collect()))));
    assert_eq!(c.now.get(), 500);
    // One check that never answers: refused at the bound, not at the client's timeout.
    let c = Clock::default();
    let hung = async |i: usize| -> Result<Option<usize>, String> {
        if i == 6 {
            std::future::pending::<()>().await;
        }
        c.open(i, 100, false).await
    };
    let got = c.run(check_in_time([0], |_| (0..20).map(hung), |d| c.sleep(d.as_millis() as u64)));
    assert_eq!((got, c.now.get()), (Ok(ChainCheck::Late), bound));
    // Checks that end just inside the bound are taken; just past it, refused.
    let c = Clock::default();
    let got = c.run(check_in_time([0], |_| [c.open(0, bound - 1, false)], |d| c.sleep(d.as_millis() as u64)));
    assert_eq!(got, Ok(ChainCheck::Checked((0, vec![0]))));
    let c = Clock::default();
    let got = c.run(check_in_time([0], |_| [c.open(0, bound + 1, false)], |d| c.sleep(d.as_millis() as u64)));
    assert_eq!(got, Ok(ChainCheck::Late));
    // A missing pack declines (the shard answers instead), and an error is an error, both before the bound.
    let c = Clock::default();
    let got = c.run(check_in_time([0], |_| (0..20).map(|i| c.open(i, 100, i == 9)), |d| c.sleep(d.as_millis() as u64)));
    assert_eq!(got, Ok(ChainCheck::Declined));
    let c = Clock::default();
    let got: Result<ChainCheck<(u8, Vec<usize>)>, String> = c.run(check_in_time(
        [0],
        |_| [async { Err::<Option<usize>, _>("r2 down".to_owned()) }],
        |d| c.sleep(d.as_millis() as u64),
    ));
    assert_eq!(got, Err("r2 down".into()));
}

/// A manifest's chain packs are checked in the network's chain buckets, home first, and the answer comes from the
/// first bucket that holds every one (`decisions/DECISIONS.md`, "`chain-warm`"). On B0-34 mt10 a fold's step deleted
/// the old chain's files from the home bucket, the home shard's kill stopped it before bucket b, and site b's shards
/// named the old chain for eight minutes: checked in the home bucket alone, all 284 kill and second misses declined
/// and composed on the shard, 5.5 s p50 against 2.0 s from R2.
#[test]
fn a_manifests_chain_is_checked_in_every_chain_bucket_home_first() {
    use graftyard_git::compose::{CHAIN_CHECK_WITHIN, ChainCheck, check_in_time};
    use graftyard_git::{chain_buckets, warm_sites};
    use std::future::Future;
    // Bucket `b` holds packs 0..20 except those in `missing[b]`; each check takes 100 ms.
    fn checks<'a>(c: &'a Clock, missing: [&'a [usize]; 2]) -> impl FnMut(usize) -> Vec<ChainOpen<'a>> {
        move |b| (0..20).map(|i| Box::pin(c.open(i, 100, missing[b].contains(&i))) as ChainOpen<'a>).collect()
    }
    type ChainOpen<'a> = std::pin::Pin<Box<dyn Future<Output = Result<Option<usize>, String>> + 'a>>;
    // The named lifetime is needed: elided, `'_` in the `Fn` sugar's return names no input (E0106).
    #[allow(clippy::needless_lifetimes)]
    fn sleep<'a>(c: &'a Clock) -> impl FnOnce(std::time::Duration) -> std::pin::Pin<Box<dyn Future<Output = ()> + 'a>> {
        move |d| Box::pin(c.sleep(d.as_millis() as u64))
    }
    // mt10: the home bucket lacks the old chain's first packs, the other holds it whole: served from the other.
    let c = Clock::default();
    let got = c.run(check_in_time([0, 1], checks(&c, [&[0, 1, 2, 3], &[]]), sleep(&c)));
    assert_eq!(got, Ok(ChainCheck::Checked((1, (0..20).collect()))), "the chain from the bucket that holds it");
    // Both hold it: the home bucket, and the other is never asked.
    let c = Clock::default();
    let got = c.run(check_in_time([0, 1], checks(&c, [&[], &[]]), sleep(&c)));
    assert_eq!((got, c.started.get()), (Ok(ChainCheck::Checked((0, (0..20).collect()))), 20));
    // Each bucket lacks a different pack: no bucket holds the chain whole, so the shard answers. Packs are never
    // taken from two buckets, since an answer streams every pack from the bucket that checked it.
    let c = Clock::default();
    let got = c.run(check_in_time([0, 1], checks(&c, [&[5], &[12]]), sleep(&c)));
    assert_eq!(got, Ok(ChainCheck::Declined));
    // The bound is one for both buckets: a home that declines late leaves the other only what is left.
    let bound = CHAIN_CHECK_WITHIN.as_millis() as u64;
    let c = Clock::default();
    let slow = |b: usize| [c.open(0, if b == 0 { bound - 100 } else { 200 }, b == 0)];
    assert_eq!(c.run(check_in_time([0, 1], slow, sleep(&c))), Ok(ChainCheck::Late));
    assert_eq!(c.now.get(), bound);

    // The chain buckets are the warm sites among the group's record, home first, once each.
    let sites = [SiteId(0), SiteId(1), SiteId(2)];
    for n in 1..200 {
        for home in sites {
            let [h, next] = warm_sites(net(n), home, &sites);
            let mut order = vec![home];
            order.extend(sites.iter().copied().filter(|s| *s != home));
            assert_eq!(chain_buckets(net(n), &tri(&order)), vec![h, next]);
            assert_eq!(h, home);
            assert_ne!(next, home);
        }
    }
    assert_eq!(chain_buckets(net(7), &tri(&[SiteId(0)])), vec![SiteId(0)]);
}

/// A network's chain buckets are among its group's sites, never the install's (`chain-record`). graft-m1 has four
/// sites, a to d, and its groups sit on a, b, and c. B0-34 mt13's monorepo network 1048606, home a: main took the
/// install's four sites and named a and d, so the builder wrote 258 chain files to bucket d, a site that runs no
/// shard of the group, chain-install never ran on b or c, and the edge checked d second. Among the record's sites
/// its chain buckets are a and b. mt12's 1048605 (a, c) and mt11's 1048604 (a, b) keep theirs, which main's rule
/// also gave them by luck of the hash. Every network on every home stays inside its record: d, outside it, never.
#[test]
fn a_networks_chain_buckets_are_among_its_groups_sites_not_the_installs() {
    let (a, b, c, d) = (SiteId(0), SiteId(1), SiteId(2), SiteId(3));
    let rec = tri(&[a, b, c]);
    assert_eq!(chain_buckets(net(1048606), &rec), [a, b], "mt13's monorepo: a and b, not a and d");
    assert_eq!(chain_buckets(net(1048605), &rec), [a, c], "mt12's monorepo keeps a and c");
    assert_eq!(chain_buckets(net(1048604), &rec), [a, b], "mt11's monorepo keeps a and b");
    // The record's order and home, not the install's: home b on [b, c, d] never names a, and a later record site can
    // be the second bucket.
    let mut seconds = std::collections::BTreeSet::new();
    for n in 1..400 {
        assert!(chain_buckets(net(n), &rec).iter().all(|s| *s != d), "net {n}: d is outside the group");
        let other = chain_buckets(net(n), &tri(&[b, c, d]));
        assert_eq!(other[0], b, "net {n}: the record's home first");
        assert!(!other.contains(&a), "net {n}: a is outside that group");
        seconds.insert(other[1]);
    }
    assert_eq!(seconds, [c, d].into(), "both of that record's other sites take networks");
    // The record's home, wherever it sits in the record: home c on [a, b, c] names c first.
    let home_c = graftyard_types::PlacementRecord { home: c, ..tri(&[a, b, c]) };
    assert_eq!(chain_buckets(net(1048606), &home_c)[0], c);
}

/// `teardown-stops-runs`: a torn-down shard that held `warming` schedules no alarm. Each catch-up run puts `warming` and
/// arms the object's alarm, and its first piece starts the container, so after `/teardown` no run may start or take
/// another piece until a caller that may start the container comes. On B0-33 sab the teardown's kill failed the
/// running run, a keep-warm caller still waiting started the next one, and that run armed the alarm the teardown had
/// deleted; the alarm restarted site c's container 3 minutes after the sweep found none running
/// (`tests/am1/results/sync-ab.md`, "Close-out"). The object is modelled by its storage: `warming` and the alarm.
#[test]
fn a_torn_down_shard_that_held_warming_schedules_no_alarm() {
    #[derive(Default)]
    struct Object {
        runs: CatchUps,
        warming: bool,
        alarm: bool,
    }
    impl Object {
        // `Jobs::start`, then the run's first piece (`Jobs::run`): `warming` put and the alarm armed.
        fn start(&mut self) {
            if self.runs.start() {
                self.warming = true;
                self.alarm = true;
            }
        }
        // A waiting caller's next turn (`Jobs::wait_from`): it starts a run when the step says so.
        fn turn(&mut self, from: u64) -> CatchUpStep {
            let s = self.runs.step(from);
            if s == CatchUpStep::Start {
                self.start();
            }
            s
        }
        // `/teardown`: runs stopped, the container killed, `warming` and the alarm deleted.
        fn teardown(&mut self) {
            self.runs.tear_down();
            self.warming = false;
            self.alarm = false;
        }
    }
    let mut o = Object::default();
    // A keep-warm caller waits on run 1, as the leader's `/catchup?warm=1` did from 12:16:39Z.
    let from = o.runs.next();
    o.start();
    assert_eq!(o.turn(from + 1), CatchUpStep::Await);
    o.teardown();
    // The kill fails the running run's piece ("The container has not been started").
    o.runs.end(false);
    assert_eq!(o.turn(from + 1), CatchUpStep::Stopped, "a waiting caller starts no run after a teardown");
    assert!(!o.warming && !o.alarm, "nothing is armed to restart the container");
    // A run already past its teardown check does not start another, and the alarm's own restart is refused too.
    o.start();
    assert!(!o.warming && !o.alarm && !o.runs.running);
    // A client request resumes catch-ups: the shard starts as it would from cold.
    o.runs.resume();
    assert_eq!(o.turn(from + 1), CatchUpStep::Start);
    assert!(o.warming && o.alarm && o.runs.running);
    // A finished run still covers its callers after a teardown, so a caller it served answers at once.
    o.runs.end(true);
    o.teardown();
    assert_eq!(o.runs.step(from + 1), CatchUpStep::Done);
}

/// Header sets (`decisions/DECISIONS.md`, "`colo-gate`"): a manifest's packs come from the set kept under the version of
/// its bucket's `head` they were checked under, and every other pack is checked. Chain pack keys name commits, not
/// bytes: after a fold rewrites a key under a new head, the set of the old head never answers for it, and a pack the
/// head no longer names is checked, so a deleted pack declines as today instead of streaming from a stale header. A
/// set is put only when every check passed, and the bound is still one `CHAIN_CHECK_WITHIN`.
#[test]
fn a_header_set_answers_only_for_the_head_version_it_was_checked_under() {
    use graftyard_git::compose::{
        CHAIN_CHECK_WITHIN, ChainCheck, ChainHead, HeadObject, HeadPack, Header, HeaderSet, Manifest, ManifestPack,
        Served, SetStore, check_with_sets,
    };
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// The world: per bucket, a head (version and chain) and its pack objects (key to count, etag).
    struct Bucket {
        head: Option<(String, ChainHead)>,
        packs: BTreeMap<String, (u32, String)>,
        /// The head object's own pack records (`head-etags`), empty for a head put without them.
        recs: Vec<HeadPack>,
    }
    #[derive(Default)]
    struct Sets {
        held: RefCell<BTreeMap<(usize, String), HeaderSet>>,
        puts: RefCell<usize>,
        served: RefCell<Vec<(usize, usize)>>,
        from_head: RefCell<Vec<usize>>,
    }
    impl SetStore for Sets {
        fn enabled(&self) -> bool {
            true
        }
        async fn get(&self, b: usize, v: &str) -> Option<HeaderSet> {
            self.held.borrow().get(&(b, v.to_owned())).cloned()
        }
        fn put(&self, b: usize, v: &str, s: HeaderSet) {
            *self.puts.borrow_mut() += 1;
            self.held.borrow_mut().insert((b, v.to_owned()), s);
        }
        fn served(&self, s: Served) {
            self.served.borrow_mut().push((s.set, s.checked));
            self.from_head.borrow_mut().push(s.head);
        }
    }
    let id = |c: char| c.to_string().repeat(40);
    let key = |p: &str| format!("compose/77/{p}.pack");
    let chain = |base: char, parts: &[char]| ChainHead {
        base: id(base),
        parts: parts.iter().map(|c| id(*c)).collect(),
        tree: id('f'),
    };
    let manifest = |h: &ChainHead| Manifest {
        v: 1,
        want: id('e'),
        packs: h.packs().iter().map(|p| ManifestPack { key: key(p), n: 10, len: 100 }).collect(),
        tail_n: 1,
        tail: 5,
    };
    let bucket = |v: &str, h: &ChainHead, etag: &str| Bucket {
        head: Some((v.to_owned(), h.clone())),
        packs: h.packs().iter().map(|p| (key(p), (10, format!("{etag}-{p}")))).collect(),
        recs: vec![],
    };
    fn check<'c>(
        c: &'c Clock,
        w: &RefCell<Vec<Bucket>>,
        b: usize,
        k: String,
        n: u32,
    ) -> impl std::future::Future<Output = Result<Option<Header>, String>> + 'c {
        let got =
            w.borrow()[b].packs.get(&k).filter(|(m, _)| *m == n).map(|(_, e)| Header { n, etag: e.clone(), body: 68 });
        async move {
            c.open(0, 100, false).await?;
            Ok(got)
        }
    }
    let head = |w: &RefCell<Vec<Bucket>>, b: usize| {
        let bk = &w.borrow()[b];
        let h = bk.head.clone().map(|(v, chain)| (v, HeadObject { chain, packs: bk.recs.clone() }));
        async move { Ok::<_, String>(h) }
    };
    #[allow(clippy::needless_lifetimes)]
    fn sleep<'a>(
        c: &'a Clock,
    ) -> impl FnOnce(std::time::Duration) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        move |d| Box::pin(c.sleep(d.as_millis() as u64))
    }
    let etags = |r: &ChainCheck<(usize, Vec<(String, Header)>)>| match r {
        ChainCheck::Checked((b, ps)) => Some((*b, ps.iter().map(|(_, h)| h.etag.clone()).collect::<Vec<_>>())),
        _ => None,
    };

    // Head v1: a base and two parts. The first answer checks all three and puts the set.
    let h1 = chain('a', &['b', 'c']);
    let w = RefCell::new(vec![bucket("v1", &h1, "x")]);
    let sets = Sets::default();
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h1),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    assert_eq!(etags(&r.unwrap()).unwrap().1.len(), 3);
    assert_eq!((*sets.puts.borrow(), sets.served.borrow().clone()), (1, vec![(0, 3)]));
    // The second answer under v1 takes every pack from the set and checks none.
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h1),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    assert!(matches!(r, Ok(ChainCheck::Checked(_))));
    assert_eq!((c.started.get(), sets.served.borrow()[1]), (0, (3, 0)), "no check under the same head version");
    assert_eq!(*sets.puts.borrow(), 1, "a set that gained nothing is not put again");
    // Under the same version, a manifest that gives a pack another count is checked, not served from the set: the
    // object holds 10, so the check finds no match and the answer declines.
    let mut m11 = manifest(&h1);
    m11.packs[1].n = 11;
    let c = Clock::default();
    let r = c.run(check_with_sets([0], &m11, |b| head(&w, b), |b, k, n| check(&c, &w, b, k, n), &sets, sleep(&c)));
    assert_eq!(r, Ok(ChainCheck::Declined), "a set entry answers only for the count it was checked at");
    assert_eq!(c.started.get(), 1);

    // A rebuild rewrites the same keys with new bytes (new etags) and puts head v2. The v1 set never answers for them.
    w.borrow_mut()[0] = bucket("v2", &h1, "y");
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h1),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    let (_, got) = etags(&r.unwrap()).unwrap();
    assert!(got.iter().all(|e| e.starts_with("y-")), "a rewritten pack streams on its new etag: {got:?}");
    assert_eq!(c.started.get(), 3);

    // A fold: head v3 names a new base alone and the old files are deleted. A manifest of the old chain is checked,
    // not served from the v2 set, and declines to the shard as today.
    let h3 = chain('d', &[]);
    w.borrow_mut()[0] = bucket("v3", &h3, "z");
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h1),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    assert_eq!(r, Ok(ChainCheck::Declined), "a deleted pack is never served from an older head's set");

    // A set whose head no longer names a pack does not answer for it, even under the same version: a manifest pack
    // outside the head is checked (here, bucket v4 names only the base; the part is checked and found).
    let h4 = chain('a', &['b']);
    let mut b4 = bucket("v4", &chain('a', &[]), "q");
    b4.packs.insert(key(&format!("part-{}-{}", id('a'), id('b'))), (10, "q-part".into()));
    w.borrow_mut()[0] = b4;
    sets.held.borrow_mut().insert(
        (0, "v4".into()),
        HeaderSet {
            packs: h4.packs().iter().map(|p| (key(p), Header { n: 10, etag: "stale".into(), body: 68 })).collect(),
        },
    );
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h4),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    let (_, got) = etags(&r.unwrap()).unwrap();
    assert_eq!(got, vec!["stale".to_owned(), "q-part".to_owned()], "only the head's own pack comes from the set");
    assert_eq!(c.started.get(), 1);

    // `head-etags`: a head that records its packs answers a new version with no check and puts no set, each pack
    // on the etag the head recorded. A record answers only at the manifest's count, and only for a pack that head
    // names: a manifest pack outside the head is checked.
    let h8 = chain('a', &['b', 'c']);
    let mut b8 = bucket("v8", &h8, "r");
    b8.recs = h8.packs().iter().map(|p| HeadPack(format!("r-{p}"), 100, 10)).collect();
    w.borrow_mut()[0] = b8;
    let puts = *sets.puts.borrow();
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h8),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    let (_, got) = etags(&r.unwrap()).unwrap();
    assert!(got.iter().all(|e| e.starts_with("r-")), "{got:?}");
    assert_eq!(c.started.get(), 0, "a head that records every pack checks none");
    assert_eq!(*sets.from_head.borrow().last().unwrap(), 3);
    assert_eq!(*sets.puts.borrow(), puts, "packs from the head are not put into a set");
    // Another count than the head recorded: that pack is checked (and declines, the object holds 10).
    let mut m9 = manifest(&h8);
    m9.packs[2].n = 9;
    let c = Clock::default();
    let r = c.run(check_with_sets([0], &m9, |b| head(&w, b), |b, k, n| check(&c, &w, b, k, n), &sets, sleep(&c)));
    assert_eq!(r, Ok(ChainCheck::Declined), "a head record answers only at its own count");
    assert_eq!(c.started.get(), 1);
    // A head that names only the base records only the base: the part beyond it is checked, on its object's etag.
    let h10 = chain('a', &['b']);
    let mut b10 = bucket("v10", &chain('a', &[]), "s");
    b10.recs = vec![HeadPack("s-base".into(), 100, 10)];
    b10.packs.insert(key(&format!("part-{}-{}", id('a'), id('b'))), (10, "s-part".into()));
    w.borrow_mut()[0] = b10;
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0],
        &manifest(&h10),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    let (_, got) = etags(&r.unwrap()).unwrap();
    assert_eq!(got, vec!["s-base".to_owned(), "s-part".to_owned()]);
    assert_eq!((c.started.get(), *sets.from_head.borrow().last().unwrap()), (1, 1));

    // A failed check puts nothing; the other bucket answers and puts its own set.
    let puts = *sets.puts.borrow();
    let mut gap = bucket("v5", &h1, "g");
    gap.packs.remove(&key(&format!("part-{}-{}", id('a'), id('b'))));
    *w.borrow_mut() = vec![gap, bucket("v6", &h1, "k")];
    let c = Clock::default();
    let r = c.run(check_with_sets(
        [0, 1],
        &manifest(&h1),
        |b| head(&w, b),
        |b, k, n| check(&c, &w, b, k, n),
        &sets,
        sleep(&c),
    ));
    assert_eq!(etags(&r.unwrap()).unwrap().0, 1);
    assert!(!sets.held.borrow().contains_key(&(0, "v5".into())), "a bucket whose check failed puts no set");
    assert_eq!(*sets.puts.borrow(), puts + 1);

    // The bound is still one CHAIN_CHECK_WITHIN.
    let bound = CHAIN_CHECK_WITHIN.as_millis() as u64;
    let h = chain('a', &[]);
    let w = RefCell::new(vec![bucket("v7", &h, "s")]);
    let c = Clock::default();
    let slow = |_: usize, _: String, _: u32| async {
        c.sleep(bound + 1).await;
        Ok::<Option<Header>, String>(None)
    };
    let r = c.run(check_with_sets([0], &manifest(&h), |b| head(&w, b), slow, &Sets::default(), sleep(&c)));
    assert_eq!(r, Ok(ChainCheck::Late));
}

/// Moved load (`maint-yield`): only a client's first try (`wait` = `FIRST_TRY_WAIT_S`) on a push or fetch path, at a
/// site that is not the record's home, holds maintenance. A hedge's or later try (60 s), the home's own first try, a
/// derived promotion, a warm call, and a merge do not.
#[test]
fn only_a_first_try_away_from_home_holds_maintenance() {
    let (home, other) = (SiteId(0), SiteId(2));
    let first = Some(FIRST_TRY_WAIT_S);
    for path in ["/validate", "/upload-pack", "/info-refs", "/catchup"] {
        assert!(maint_hold(path, Caller::Client, false, other, home, first), "{path}");
        assert!(!maint_hold(path, Caller::Client, false, home, home, first), "{path} at home");
    }
    assert!(!maint_hold("/validate", Caller::Client, false, other, home, Some(60)), "a hedge's try");
    assert!(!maint_hold("/validate", Caller::Client, false, other, home, None), "no wait");
    assert!(!maint_hold("/validate", Caller::Client, false, other, home, Some(5)), "a warm ask's wait");
    assert!(!maint_hold("/validate", Caller::Client, true, other, home, first), "a derived promotion");
    assert!(!maint_hold("/catchup", Caller::Warm, false, other, home, first), "a warm call");
    assert!(!maint_hold("/catchup", Caller::PoolWarm, false, other, home, first), "a pool warm");
    assert!(!maint_hold("/promote", Caller::Start, false, other, home, first), "a due range's promotion");
    assert!(!maint_hold("/merge", Caller::Client, false, other, home, first), "a merge is not moved load");
    // One call per MAINT_HOLD_EVERY keeps the container's 60 s hold renewed with time to spare.
    assert!(MAINT_HOLD_EVERY.as_secs() * 3 <= 60);
}

/// The builder's chain step on a model container (`compose::builder_step`, `builder-install`). The container's step is
/// the container's rule: with no chain, a base at HEAD; with one, a part from its tip to HEAD. On B0-34 mt20b and mt14
/// the restarted home's container held no chain while the warm buckets held the one it put before the kill, and its
/// step rebuilt a 260 MB base for 2 and 4.5 minutes (`tests/am1/results/g37-design.md`). On main's rule the builder
/// only steps, so a new container always builds a base.
#[test]
fn a_restarted_builder_installs_the_buckets_chain_and_steps_a_part() {
    use graftyard_git::compose::{BuilderIo, ChainHead, builder_step};
    use std::cell::{Cell, RefCell};
    fn id(c: char) -> String {
        c.to_string().repeat(40)
    }
    let chain = |base: char, parts: &[char]| ChainHead {
        base: id(base),
        parts: parts.iter().map(|p| id(*p)).collect(),
        tree: id('f'),
    };
    struct Model {
        held: RefCell<Option<ChainHead>>,
        bucket: Option<ChainHead>,
        install_ok: bool,
        head: String,
        installs: Cell<usize>,
        bases: Cell<usize>,
    }
    impl BuilderIo for Model {
        async fn held(&self) -> Result<Option<ChainHead>, StoreError> {
            Ok(self.held.borrow().clone())
        }
        async fn bucket_head(&self) -> Result<Option<ChainHead>, StoreError> {
            Ok(self.bucket.clone())
        }
        async fn install(&self, head: &ChainHead) -> Result<bool, StoreError> {
            self.installs.set(self.installs.get() + 1);
            if self.install_ok {
                *self.held.borrow_mut() = Some(head.clone());
            }
            Ok(self.install_ok)
        }
        async fn step(&self) -> Result<Option<ChainHead>, StoreError> {
            let mut held = self.held.borrow_mut();
            let next = match held.take() {
                None => {
                    self.bases.set(self.bases.get() + 1);
                    ChainHead { base: self.head.clone(), parts: vec![], tree: id('f') }
                }
                Some(mut h) if h.tip() != self.head => {
                    h.parts.push(self.head.clone());
                    h
                }
                Some(h) => h,
            };
            *held = Some(next.clone());
            Ok(Some(next))
        }
    }
    let model = |held: Option<ChainHead>, bucket: Option<ChainHead>, install_ok: bool| Model {
        held: RefCell::new(held),
        bucket,
        install_ok,
        head: id('9'),
        installs: Cell::new(0),
        bases: Cell::new(0),
    };
    let step = |m: &Model| futures_executor::block_on(builder_step(m)).unwrap().unwrap();
    // mt20b's case: a new container, the buckets at the pre-kill chain (base a, parts b, c), HEAD moved to 9.
    let m = model(None, Some(chain('a', &['b', 'c'])), true);
    assert_eq!(step(&m), chain('a', &['b', 'c', '9']), "the bucket chain installed, then a part to HEAD");
    assert_eq!((m.installs.get(), m.bases.get()), (1, 0), "never a base");
    // A container that holds a chain, even one other than the buckets' (its own later step), installs nothing over it.
    let m = model(Some(chain('a', &['b', 'c', 'd'])), Some(chain('a', &['b'])), true);
    assert_eq!(step(&m), chain('a', &['b', 'c', 'd', '9']));
    assert_eq!((m.installs.get(), m.bases.get()), (0, 0), "the local chain is not replaced by an older bucket head");
    // No bucket head (the network's first warm): a base, as before.
    let m = model(None, None, true);
    assert_eq!(step(&m), ChainHead { base: id('9'), parts: vec![], tree: id('f') });
    assert_eq!((m.installs.get(), m.bases.get()), (0, 1));
    // A bucket file missing: the install fails and the step still builds a base, so the builder is never left with none.
    let m = model(None, Some(chain('a', &['b'])), false);
    assert_eq!(step(&m), ChainHead { base: id('9'), parts: vec![], tree: id('f') });
    assert_eq!((m.installs.get(), m.bases.get()), (1, 1));
}
