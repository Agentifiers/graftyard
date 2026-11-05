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

#[test]
fn a_running_container_restarts_only_when_the_image_map_names_another_image() {
    let (old, new) = ("registry/git@sha256:b7bb", "registry/git@sha256:aa30");
    assert!(image_restart(true, Some(old), new), "a new version's image rolls a running container");
    assert!(!image_restart(true, Some(new), new), "the same image keeps it");
    assert!(image_restart(true, None, new), "a container started before shards recorded their image rolls once");
    assert!(!image_restart(false, Some(old), new), "a stopped container starts on the new image anyway");
}
