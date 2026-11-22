//! M5-WP4 in memory: the gate inputs (`crate::gate`), automatic drain and undrain by the drain key, an automatic drain
//! preempting a site day, `STUCK_DAY`, `STEP_MIN_ENTRIES`, the 1% lead limit, and `ContainerDay` with its rollback,
//! each driven through `ops::next` against the real group. BF-39's kill row is in `run.rs`.

use std::collections::BTreeMap;

use super::ops::{OLD, Ops, upload_all, version_of};
use super::*;
use crate::gate::{self, HealthLog, Side, SiteHealth, Verdict};
use crate::ops::{self, Action, Dwell, Gate, Signer};

const BURNING: Verdict = Verdict { burning: true, calm: false };
const CALM: Verdict = Verdict { burning: false, calm: true };

fn health(v: &[(u8, Verdict)]) -> BTreeMap<u8, Verdict> {
    v.iter().copied().collect()
}

fn next(o: &Ops) -> Action {
    ops::next(&o.g.f, &o.world(), Dwell::default())
}

fn mode(a: &Action) -> Option<(u8, Mode)> {
    match a {
        Action::Commit { body: ReleaseEntry::SiteMode(m), signer: Signer::Drain } => Some((m.site.0, m.mode)),
        _ => None,
    }
}

/// Every `SiteMode` committed after log position `from`: site and mode.
fn modes_since(o: &Ops, from: usize) -> Vec<(u8, Mode)> {
    o.g.log[from..]
        .iter()
        .filter_map(|b| match SignedEntry::decode(b).unwrap().body {
            ReleaseEntry::SiteMode(m) => Some((m.site.0, m.mode)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_step_gate_breaches_on_each_input_at_its_bound_and_starves_under_step_min_entries() {
    let n = Side { apply_lag_p99_ms: 100, exceptions: 0, ack_bp: 9_990, step_downs: 0, entries: 5_000 };
    let ok = Side { apply_lag_p99_ms: 200, ack_bp: 9_980, entries: gate::STEP_MIN_ENTRIES, ..n };
    assert_eq!(gate::judge_site(&ok, &n), Gate::Pass, "twice N's lag, 0.1 points of ack rate, 1,000 entries");
    let breach = |s: Side| matches!(gate::judge_site(&s, &n), Gate::Breach(_));
    assert!(breach(Side { apply_lag_p99_ms: 201, ..ok }), "over twice N's p99");
    assert!(breach(Side { ack_bp: 9_979, ..ok }), "more than 0.1 points under N's ack rate");
    assert!(breach(Side { exceptions: 1, ..ok }));
    assert!(breach(Side { step_downs: 1, ..ok }));
    // The 500 ms cap holds even when N is slow too.
    let slow = Side { apply_lag_p99_ms: 400, ..n };
    assert_eq!(gate::judge_site(&Side { apply_lag_p99_ms: 500, ..ok }, &slow), Gate::Pass);
    assert!(matches!(gate::judge_site(&Side { apply_lag_p99_ms: 501, ..ok }, &slow), Gate::Breach(_)));
    // A quiet step is starved, not passed; a breach still wins over too few entries.
    assert_eq!(gate::judge_site(&Side { entries: 999, ..ok }, &n), Gate::Starved(999));
    assert!(breach(Side { entries: 10, exceptions: 1, ..ok }));
    assert_eq!(gate::judge_errors(20, 10), Gate::Pass);
    assert!(matches!(gate::judge_errors(21, 10), Gate::Breach(_)));
}

/// The host's counters: summed across isolates, read back as the p99 bucket's upper bound and the ack rate; with no N
/// baseline (a site's first release, or 100% where no member runs N) only the 500 ms cap and the absolute inputs hold.
#[test]
fn counters_sum_across_isolates_into_the_side_the_gate_judges_and_no_baseline_leaves_only_the_cap() {
    let mut a = gate::Counters::default();
    for _ in 0..990 {
        a.applied(40);
    }
    let mut b = gate::Counters { appends: 1_000, acks: 998, ..Default::default() };
    for _ in 0..9 {
        b.applied(40);
    }
    b.applied(260); // the 1,000th entry: in the 300 ms bucket, past the 99th percentile
    a.add(&b);
    let side = a.side();
    assert_eq!((side.entries, side.ack_bp), (1_000, 9_980));
    assert_eq!(side.apply_lag_p99_ms, 50, "990 + 9 of 1,000 entries at 40 ms: p99 is the 50 ms bucket");
    a.applied(260);
    assert_eq!(a.side().apply_lag_p99_ms, 50, "2 of 1,001 above the bucket still leave p99 in it");
    for _ in 0..9 {
        a.applied(260);
    }
    assert_eq!(a.side().apply_lag_p99_ms, 300, "11 of 1,010 above: p99 moves to their bucket");
    assert_eq!(gate::Counters::default().side().ack_bp, 10_000, "no AppendEntries answered is not a 0% ack rate");
    // No N: a lag that would be over twice N's, and an ack rate far under it, pass; the cap still breaches.
    let none = gate::Counters::default().side();
    let n1 = Side { apply_lag_p99_ms: 400, exceptions: 0, ack_bp: 9_000, step_downs: 0, entries: 5_000 };
    assert_eq!(gate::judge_site(&n1, &none), Gate::Pass);
    assert!(matches!(gate::judge_site(&Side { apply_lag_p99_ms: 501, ..n1 }, &none), Gate::Breach(_)));
    assert!(matches!(gate::judge_site(&Side { exceptions: 1, ..n1 }, &none), Gate::Breach(_)));
}

/// `day_versions` names the rolling site with the intent's version as N+1 and the running release's as N, only while
/// a site day runs.
#[test]
fn day_versions_names_the_site_days_n1_and_n_versions_only_while_a_site_day_runs() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    assert_eq!(gate::day_versions(&o.g.f), None);
    o.roll_all(1);
    upload_all(&mut o.g, 2);
    o.next_id += 1;
    let b = o.g.intent(o.next_id, super::ops::site_day(1, 2), &[], &VENDOR);
    o.g.ok(b);
    let s = Script::Site(SiteId(1));
    assert_eq!(gate::day_versions(&o.g.f), Some((SiteId(1), version_of(2, s), Some(version_of(1, s)))));
    o.finish();
    assert_eq!(gate::day_versions(&o.g.f), None, "no site day current");
}

#[test]
fn a_site_burns_only_when_both_other_sites_report_it_and_each_report_counts_once_it_has_min_calls() {
    let voters = [SiteId(0), SiteId(1), SiteId(2)];
    let h =
        |o: u8, t: u8, calls: u64, missed: u64| SiteHealth { observer: SiteId(o), target: SiteId(t), calls, missed };
    // 10 x the allowed 1 in 2,000 is 10 missed in 2,000.
    let both = [h(0, 2, 2_000, 10), h(1, 2, 2_000, 10)];
    assert!(gate::verdict(&voters, &both, &[])[&2].burning);
    let under = [h(0, 2, 2_000, 10), h(1, 2, 2_000, 9)];
    assert!(!gate::verdict(&voters, &under, &[])[&2].burning, "one observer under DRAIN_BURN");
    let one = [h(0, 2, 2_000, 2_000)];
    assert!(!gate::verdict(&voters, &one, &[])[&2].burning, "one observer alone");
    // The sick site's own report about itself or the others decides nothing.
    let own = [h(2, 2, 2_000, 0), h(0, 2, 2_000, 10), h(1, 2, 2_000, 10), h(2, 0, 2_000, 2_000)];
    let v = gate::verdict(&voters, &own, &[]);
    assert!(v[&2].burning && !v[&0].burning);
    let few = [h(0, 2, 99, 99), h(1, 2, 99, 99)];
    assert!(!gate::verdict(&voters, &few, &[])[&2].burning, "under MIN_CALLS is no evidence");
    // Calm needs both under 1 x over the slow window: 1 missed in 2,000 is 1 x, not under it.
    let calm = [h(0, 2, 2_000, 0), h(1, 2, 2_001, 1)];
    assert!(gate::verdict(&voters, &[], &calm)[&2].calm);
    let at = [h(0, 2, 2_000, 0), h(1, 2, 2_000, 1)];
    assert!(!gate::verdict(&voters, &[], &at)[&2].calm);
    assert!(!gate::verdict(&voters, &[], &few)[&2].calm, "no evidence is not calm");
}

#[test]
fn a_health_log_sums_its_window_and_keeps_only_undrain_after() {
    let mut l = HealthLog::default();
    let t0 = 10_000_000;
    l.add(t0, SiteId(2), 100, 50);
    l.add(t0 + gate::DRAIN_WINDOW, SiteId(2), 1_000, 1);
    l.add(t0 + gate::DRAIN_WINDOW, SiteId(1), 7, 7);
    let r = l.report(SiteId(0), SiteId(2), t0 + gate::DRAIN_WINDOW + 999, gate::DRAIN_WINDOW);
    assert_eq!((r.calls, r.missed), (1_100, 51), "the window's first second included");
    let r = l.report(SiteId(0), SiteId(2), t0 + gate::DRAIN_WINDOW + 1_000, gate::DRAIN_WINDOW);
    assert_eq!((r.calls, r.missed), (1_000, 1), "one second later the oldest second is out");
    l.add(t0 + gate::UNDRAIN_AFTER + 2_000, SiteId(2), 1, 0);
    assert!(l.secs.keys().all(|(s, _)| *s * 1000 > t0), "seconds past UNDRAIN_AFTER are pruned");
}

/// With nothing current, a site both other sites call burning is drained by the drain key, stays drained while it
/// burns, and is undrained by the same key once both call it calm.
#[test]
fn graft_ops_drains_a_burning_site_with_the_drain_key_and_undrains_it_once_calm() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.health = health(&[(2, BURNING)]);
    assert_eq!(mode(&next(&o)), Some((2, Mode::Drain)));
    o.act();
    assert!(o.g.f.drained[&2].by_drain_key);
    assert_eq!(next(&o), Action::Idle, "drained and still burning: nothing to do");
    o.health = health(&[(2, Verdict::default())]);
    assert_eq!(next(&o), Action::Idle, "neither burning nor calm: it stays drained");
    o.health = health(&[(2, CALM)]);
    assert_eq!(mode(&next(&o)), Some((2, Mode::Normal)));
    o.act();
    assert!(o.g.f.drained.is_empty());
    // A site an intent key drained is a human's: a burning other site pages, since the group refuses a second drain.
    let b = o.g.drain(0, Mode::Drain, &CUSTOMER);
    o.g.ok(b);
    o.health = health(&[(0, CALM), (2, BURNING)]);
    assert!(matches!(next(&o), Action::Page(why) if why.contains("site 0 is drained")));
}

/// Journey 5: an automatic drain preempts a site day. `graft-ops` stops the deploy steps, appends `Normal` for the
/// rolling site, drains the sick one, and the day goes on under the 1% lead limit without draining its site again.
#[test]
fn an_automatic_drain_preempts_a_site_day_under_the_lead_limit() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    upload_all(&mut o.g, 2);
    assert!(matches!(o.roll(2), ops::Roll::Next { .. }));
    assert_eq!(gate::lead_limited(&o.g.f), None, "before its drain step the day limits nothing");
    // A day that starts while another site is drained waits to drain its own, which the group would refuse.
    o.health = health(&[(1, BURNING)]);
    o.act();
    o.health = BTreeMap::new();
    assert!(matches!(next(&o), Action::Wait { until: None, why } if why.contains("site 1 is drained")));
    o.health = health(&[(1, CALM)]);
    o.act();
    o.health = BTreeMap::new();
    while o.g.f.current.as_deref().unwrap().next < 2 {
        o.act();
    }
    o.act(); // site 0 at 10%
    let from = o.g.log.len();
    let deployed = o.deployed.len();
    assert_eq!(gate::lead_limited(&o.g.f), None, "drained for its day, not lead-limited");
    // The day's own drain is not a calm site's to end.
    o.health = health(&[(0, CALM)]);
    assert_eq!(mode(&next(&o)), None);
    o.health = health(&[(0, CALM), (2, BURNING)]);
    assert_eq!(mode(&next(&o)), Some((0, Mode::Normal)), "the rolling site undrains first");
    o.act();
    assert_eq!(mode(&next(&o)), Some((2, Mode::Drain)));
    o.act();
    assert_eq!(gate::lead_limited(&o.g.f), Some(SiteId(0)));
    let limited = (0..10_000).filter(|i| gate::may_not_lead(&o.g.f, &format!("repo:{i}")).contains(&SiteId(0))).count();
    assert!((9_860..=9_940).contains(&limited), "site 0 may lead about 1% of chains: {limited} limited");
    assert!((0..1_000).all(|i| gate::may_not_lead(&o.g.f, &format!("repo:{i}")).contains(&SiteId(2))));
    assert!(matches!(next(&o), Action::Wait { until: None, why } if why.contains("site 2 is drained")));
    // Site 2 recovers: undrained, then the day runs to its end with no second drain of site 0.
    o.health = health(&[(2, CALM)]);
    assert_eq!(mode(&next(&o)), Some((2, Mode::Normal)));
    o.finish();
    assert_eq!(modes_since(&o, from), vec![(0, Mode::Normal), (2, Mode::Drain), (2, Mode::Normal)]);
    assert_eq!(o.deployed.len(), deployed + 2, "50% and 100%, nothing while site 2 was drained");
    let s0 = Script::Site(SiteId(0));
    assert_eq!(o.deps[&s0].split, vec![(version_of(2, s0), 100)]);
    assert_eq!(o.g.f.site_release.get(&0), Some(&rel(2).0));
    assert_eq!(gate::lead_limited(&o.g.f), None, "the limit ends with the day");
}

/// `STUCK_DAY`: a site day that cannot advance for an hour, on an unreadable deployment or a gate that stays pending,
/// undrains its site, and goes on from the same step once it can.
#[test]
fn a_site_day_stuck_an_hour_undrains_its_site_and_resumes() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    assert!(matches!(o.roll(1), ops::Roll::Next { .. }));
    while o.g.f.current.as_deref().unwrap().next < 1 {
        o.act();
    }
    o.act(); // the 1% deploy
    let s0 = Script::Site(SiteId(0));
    let dep = o.deps.remove(&s0).unwrap();
    let since = o.g.f.current.as_deref().unwrap().since;
    o.g.at = Timestamp(since.0 + gate::STUCK_DAY - 1);
    assert!(matches!(next(&o), Action::Wait { until: None, .. }));
    o.g.at = Timestamp(since.0 + gate::STUCK_DAY);
    assert_eq!(mode(&next(&o)), Some((0, Mode::Normal)));
    o.act();
    assert_eq!(gate::lead_limited(&o.g.f), Some(SiteId(0)));
    o.deps.insert(s0, dep);
    o.finish();
    assert_eq!(o.g.f.site_release.get(&0), Some(&rel(1).0));
    assert!(o.g.f.drained.is_empty());

    // A gate that stays pending for an hour after the deploy does the same.
    assert!(matches!(o.roll(1), ops::Roll::Next { .. }));
    while o.g.f.current.as_deref().unwrap().next < 1 {
        o.act();
    }
    o.act();
    o.gate = Gate::Pending;
    let deployed = o.deps[&Script::Site(SiteId(1))].created;
    o.g.at = Timestamp(deployed.0 + gate::STUCK_DAY - 1);
    assert!(matches!(next(&o), Action::Wait { until: None, why } if why.contains("pending")));
    o.g.at = Timestamp(deployed.0 + gate::STUCK_DAY);
    assert_eq!(mode(&next(&o)), Some((1, Mode::Normal)));
}

/// Under `STEP_MIN_ENTRIES` a step dwells on up to twice `STEP_DWELL`, passes if the entries come, and pages if not.
#[test]
fn a_starved_step_dwells_twice_its_dwell_then_pages() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    assert!(matches!(o.roll(1), ops::Roll::Next { .. }));
    while o.g.f.current.as_deref().unwrap().next < 1 {
        o.act();
    }
    o.act(); // the 1% deploy
    let start = o.deps[&Script::Site(SiteId(0))].created;
    o.gate = Gate::Starved(10);
    o.g.at = Timestamp(start.0 + ops::STEP_DWELL);
    assert_eq!(
        next(&o),
        Action::Wait { until: Some(Timestamp(start.0 + 2 * ops::STEP_DWELL)), why: "STEP_MIN_ENTRIES".into() }
    );
    o.g.at = Timestamp(start.0 + 2 * ops::STEP_DWELL);
    assert!(matches!(next(&o), Action::Page(why) if why.contains("STEP_MIN_ENTRIES")));
    // The entries arrive before the cap: the step passes.
    o.g.at = Timestamp(start.0 + 3 * ops::STEP_DWELL / 2);
    o.gate = Gate::Pass;
    assert!(matches!(next(&o), Action::Commit { body: ReleaseEntry::IntentStep { step: 1, .. }, .. }));
}

/// A `ContainerDay` restarts 10%, 50%, then 100% of the site's shards through the view alone, with no deployment
/// change; its rollback returns every shard to `git_prev`, also with no API call.
#[test]
fn a_container_day_rolls_shards_through_the_view_and_its_rollback_needs_no_api() {
    let mut o = Ops::new(G::new());
    upload_all(&mut o.g, 1);
    o.roll_all(1);
    assert_eq!(gate::shard_day(&o.g.f, SiteId(0)), None, "no ContainerDay yet: git_prev");
    o.deps.clear(); // the API answers nothing for the whole day
    let deployed = o.deployed.len();
    o.next_id += 1;
    let b = o.g.intent(o.next_id, Intent::ContainerDay { site: SiteId(0), release: rel(1).0 }, &[], &VENDOR);
    o.g.ok(b);
    let mut seen = Vec::new();
    while o.g.f.current.is_some() {
        seen.push(gate::shard_day(&o.g.f, SiteId(0)));
        o.act();
    }
    seen.dedup();
    assert_eq!(seen, vec![Some(0), Some(1), Some(2)]);
    assert_eq!(o.deployed.len(), deployed, "no deployment change");
    assert_eq!(gate::shard_day(&o.g.f, SiteId(0)), Some(2), "every shard on the release's image");
    assert_eq!(gate::shard_day(&o.g.f, SiteId(1)), None, "another site's shards stay on git_prev");
    // The next release's ContainerDay breaches at 50%: its pre-signed rollback returns every shard to git_prev at once.
    upload_all(&mut o.g, 2);
    o.next_id += 1;
    let undo = ReleaseEntry::Rollback { target: DayTarget::Containers(SiteId(0)), release: rel(1).0 };
    let b = o.g.intent(o.next_id, Intent::ContainerDay { site: SiteId(0), release: rel(2).0 }, &[undo], &VENDOR);
    o.g.ok(b);
    o.act();
    o.g.at = Timestamp(o.g.at.0 + ops::STEP_DWELL);
    o.act(); // step 0 done
    assert_eq!(gate::shard_day(&o.g.f, SiteId(0)), Some(1));
    o.gate = Gate::Breach("GATE_EXCEPTIONS".into());
    assert!(matches!(o.act(), Action::Undo(_)));
    assert_eq!(gate::shard_day(&o.g.f, SiteId(0)), None, "rolled back: git_prev");
    o.gate = Gate::Pass;
    o.finish();
    assert_eq!(o.g.f.containers.get(&0), Some(&rel(1).0));
    assert_eq!(gate::shard_day(&o.g.f, SiteId(0)), Some(2), "release 1's image, which the site's version names git");
    assert_eq!(o.deployed.len(), deployed);
    // A site day to a newer release makes the older image `git_prev` again until that release's ContainerDay.
    o.deps.insert(Script::Site(SiteId(0)), ops::Deployment { split: vec![(OLD, 100)], created: o.g.at });
    let mut b = o.g.f.clone();
    b.site_release.insert(0, rel(2).0);
    assert_eq!(gate::shard_day(&b, SiteId(0)), None);
}
