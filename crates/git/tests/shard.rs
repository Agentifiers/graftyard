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
fn first_shard_is_in_count_and_the_rest_in_the_hot_pool_without_repeats() {
    for count in SHARDS {
        for n in 0..500 {
            for k in 1..=FANOUT_MAX {
                let s = shards_for(net(n), SiteId(1), count, k);
                assert_eq!(s.len(), k as usize);
                assert!(s[0].shard < count);
                assert!(s[1..].iter().all(|x| (count..count + HOT_POOL).contains(&x.shard)));
                let mut v: Vec<u16> = s.iter().map(|x| x.shard).collect();
                v.sort();
                v.dedup();
                assert_eq!(v.len(), s.len());
                assert!(s.iter().all(|x| x.site == SiteId(1) && may_start(x.shard, count)));
            }
        }
    }
    // k = 0 is clamped to one shard, and more than FANOUT_MAX to FANOUT_MAX.
    assert_eq!(shards_for(net(1), SiteId(0), 5, 0).len(), 1);
    assert_eq!(shards_for(net(1), SiteId(0), 5, 200).len(), FANOUT_MAX as usize);
}

#[test]
fn raising_k_moves_no_network_and_keeps_the_earlier_shards() {
    for n in 0..300 {
        let base = shards_for(net(n), SiteId(0), 25, 1)[0];
        for k in 2..=FANOUT_MAX {
            let lo = shards_for(net(n), SiteId(0), 25, k - 1);
            let hi = shards_for(net(n), SiteId(0), 25, k);
            assert_eq!(hi[0], base);
            assert_eq!(&hi[..lo.len()], &lo[..], "k-1 shards are a prefix of k");
        }
    }
}

#[test]
fn rendezvous_spreads_networks_and_a_new_count_moves_only_about_one_in_n() {
    let mut load: BTreeMap<u16, u32> = BTreeMap::new();
    for n in 0..25_000 {
        *load.entry(shards_for(net(n), SiteId(2), 25, 1)[0].shard).or_default() += 1;
    }
    assert_eq!(load.len(), 25);
    let (min, max) = (load.values().min().unwrap(), load.values().max().unwrap());
    assert!(*min > 850 && *max < 1150, "{min}..{max}");
    // From 25 to 26 shards, about 1/26 of networks move, and only onto the new shard.
    let mut moved = 0;
    for n in 0..25_000 {
        let a = shards_for(net(n), SiteId(2), 25, 1)[0];
        let b = shards_for(net(n), SiteId(2), 26, 1)[0];
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
    let k = shards_for(net(9), SiteId(0), 5, 4);
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

/// A new leadership's warm catch-up starts no container and keeps none warm: on `graft-m2b`, groups with no client
/// re-elected every hour or so, and each `warm` started its network's shard, which `WARM_FOR` then held for a day
/// (`tests/am1/results/idle-shards.md`). A client's call still starts a stopped shard and keeps it warm.
#[test]
fn only_a_clients_call_starts_a_shard_or_keeps_it_warm() {
    assert_eq!(serve(Caller::Warm, false), Serve::Skip);
    assert_eq!(serve(Caller::Warm, true), Serve::Run { touch: false });
    assert_eq!(serve(Caller::Client, false), Serve::Run { touch: true });
    assert_eq!(serve(Caller::Client, true), Serve::Run { touch: true });
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
    DiskRepo { network: net(n), bytes: mb << 20, used: Timestamp(used) }
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
