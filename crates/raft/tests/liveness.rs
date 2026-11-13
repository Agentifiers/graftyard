//! Liveness under gray faults, in numbers: after a fault starts on the leader's site, how long until a leader on
//! another site commits a client write, and how many leader changes it took. Compared with Milestone 0's B0-06
//! (`tests/m0/results/m0-wp4.md`): leaders left a site failing 30% of calls in 2.9 to 4.1 s and slow storage in 2.8
//! to 4.2 s, and failing writes moved by election in 4.9 to 13.3 s.
//!
//! `cargo test -p graftyard-raft --test liveness` runs `SEEDS` seeds per fault; `RAFT_LIVE_SEEDS=n` with `--ignored`
//! runs the long version.

mod common;

use common::sim::{Edge, FaultKind, Faults, SITES, Sim, SiteFault};
use graftyard_raft::Role;
use graftyard_types::{SiteId, Timestamp};

const SEEDS: u64 = 12;
/// Clients: about 20 acked writes a second on the group, near Milestone 0's 25 per group (100 writes/s over 4).
const CLIENTS: usize = 16;
/// Long enough for a group elected away from `home` to transfer back (one STEP_DOWN window after it serves).
const WARM_MS: u64 = 30_000;
const FAULT_MS: u64 = 60_000;
/// The design's gray-region target: every group moved within 20 s.
const TARGET_MS: u64 = 20_000;

struct Run {
    /// From the fault's start to the first write acked by a leader off the faulty site.
    moved_ms: Option<u64>,
    /// From the fault's start to the first leader term off the faulty site: Milestone 0's "moved" (first sample with
    /// a leader off A).
    led_ms: Option<u64>,
    acks_per_s: f64,
    /// The sick leader's storage confirms per second, from the fault to its move: the confirm rule's input rate.
    sick_confirms_per_s: f64,
    /// Leader terms that started after the fault did.
    changes: usize,
    /// How the faulty site's leadership ended: by a `STEP_DOWN` transfer, or otherwise (election).
    by_step_down: bool,
    linearizable: bool,
}

fn run(seed: u64, kind: FaultKind) -> Run {
    let clients = std::env::var("RAFT_LIVE_CLIENTS").ok().and_then(|v| v.parse().ok()).unwrap_or(CLIENTS);
    let mut sim = Sim::new(seed, Faults::WAN, clients, 3);
    sim.record_confirms = true;
    let warm = Timestamp(sim.now.0 + WARM_MS);
    while sim.now < warm {
        sim.step(true);
    }
    let sick = leader_now(&sim).unwrap_or_else(|| {
        let roles: Vec<_> = common::sim::SITES
            .iter()
            .map(|s| sim.member(*s).map(|m| (m.role(), m.term(), m.state().commit, m.log().len())))
            .collect();
        panic!(
            "seed {seed}: no leader after warm-up: {roles:?} acked {} terms {:?} alarms {:?}",
            sim.stats.acked, sim.terms, sim.alarms
        )
    });
    let start = sim.now;
    sim.site_faults.push(SiteFault { site: sick, from: start, until: Timestamp(start.0 + FAULT_MS), kind });
    let terms_before = sim.terms.len();
    let end = Timestamp(start.0 + FAULT_MS);
    while sim.now < end {
        sim.step(true);
    }
    sim.settle(4_000);
    let moved_ms = sim.acks.iter().find(|(t, s)| *t >= start && *s != sick).map(|(t, _)| t.0 - start.0);
    let led_ms = sim.terms.iter().skip(terms_before).find(|t| t.1 != sick).map(|t| t.2.0 - start.0);
    let warm_acks = sim.acks.iter().filter(|(t, _)| t.0 + WARM_MS / 2 >= start.0 && *t < start).count();
    let acks_per_s = warm_acks as f64 / (WARM_MS as f64 / 2_000.0);
    let until = Timestamp(start.0 + led_ms.unwrap_or(FAULT_MS));
    let sick_confirms = sim.confirm_log.iter().filter(|c| c.0 >= start && c.0 < until && c.1 == sick && c.3).count();
    let sick_confirms_per_s = sick_confirms as f64 * 1000.0 / (until.0 - start.0).max(1) as f64;
    let by_step_down = sim
        .alarms
        .iter()
        .any(|(t, s, a)| *t >= start && *s == sick && matches!(a, graftyard_raft::Alarm::StepDown { to: Some(_), .. }));
    let linearizable = common::history::check(&sim.history) == common::history::Verdict::Ok;
    if std::env::var("RAFT_LIVE_TRACE").is_ok() {
        let rel = |t: Timestamp| t.0 as i64 - start.0 as i64;
        let alarms: Vec<_> = sim.alarms.iter().filter(|a| a.0 >= start).map(|a| (rel(a.0), a.1, &a.2)).collect();
        let terms: Vec<_> = sim.terms.iter().skip(terms_before).map(|t| (t.0, t.1, rel(t.2))).collect();
        eprintln!("seed {seed} sick {sick:?} moved {moved_ms:?}\n  terms {terms:?}\n  alarms {alarms:?}");
    }
    Run {
        moved_ms,
        led_ms,
        acks_per_s,
        sick_confirms_per_s,
        changes: sim.terms.len() - terms_before,
        by_step_down,
        linearizable,
    }
}

fn leader_now(sim: &Sim) -> Option<SiteId> {
    sim.leader().filter(|s| sim.member(*s).is_some_and(|m| m.role() == Role::Leader))
}

fn report(name: &str, kind: FaultKind, seeds: std::ops::Range<u64>) -> Vec<Run> {
    let only: Option<u64> = std::env::var("RAFT_LIVE_ONLY").ok().and_then(|v| v.parse().ok());
    let runs: Vec<Run> = seeds.filter(|s| only.is_none_or(|o| o == *s)).map(|s| run(s, kind)).collect();
    let dist = |mut v: Vec<u64>| {
        v.sort_unstable();
        let pct = |p: usize| v.get((v.len().max(1) - 1) * p / 100).copied().unwrap_or(0) as f64 / 1000.0;
        format!("{} runs: min {:.1} p50 {:.1} p90 {:.1} max {:.1} s", v.len(), pct(0), pct(50), pct(90), pct(100))
    };
    let mut changes: Vec<usize> = runs.iter().map(|r| r.changes).collect();
    changes.sort_unstable();
    let n = runs.len().max(1) as f64;
    let load = runs.iter().map(|r| r.acks_per_s).sum::<f64>() / n;
    let confirms = runs.iter().map(|r| r.sick_confirms_per_s).sum::<f64>() / n;
    eprintln!(
        "{name} ({load:.1} acked writes/s before the fault; sick leader {confirms:.1} confirms/s):\n  \
         new leader off the site  {}\n  healthy leader commits   {}\n  \
         moved by STEP_DOWN {} of {} | leader changes per run {:?}",
        dist(runs.iter().filter_map(|r| r.led_ms).collect()),
        dist(runs.iter().filter_map(|r| r.moved_ms).collect()),
        runs.iter().filter(|r| r.by_step_down).count(),
        runs.len(),
        changes,
    );
    for r in &runs {
        assert!(r.linearizable, "{name}: history is not linearizable");
    }
    runs
}

/// Every failed condition, by seed, so a long run reports all of them.
fn check_all(name: &str, runs: &[Run], by_step_down: bool, first_seed: u64) -> Vec<String> {
    let mut out = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        let seed = first_seed + i as u64;
        match r.moved_ms {
            None => out.push(format!("{name} seed {seed}: no healthy leader committed in the fault")),
            Some(ms) if ms > TARGET_MS => out.push(format!("{name} seed {seed}: moved in {ms} ms")),
            _ => {}
        }
        if r.changes > 2 {
            out.push(format!("{name} seed {seed}: {} leader changes", r.changes));
        }
    }
    // The design moves a gray leader by STEP_DOWN or by `suspect`; STEP_DOWN should do most of it.
    let by = runs.iter().filter(|r| r.by_step_down).count();
    if by_step_down && by * 10 < runs.len() * 9 {
        out.push(format!("{name}: only {by} of {} moved by STEP_DOWN", runs.len()));
    }
    out
}

fn check(name: &str, runs: &[Run], by_step_down: bool) {
    let failed = check_all(name, runs, by_step_down, 0);
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn leaders_leave_a_site_whose_calls_fail() {
    let runs = report("calls 30%", FaultKind::Calls(300), 100..100 + SEEDS);
    check("calls", &runs, true);
}

#[test]
fn leaders_leave_slow_storage() {
    let runs = report("slow confirms 30% at 1 s", FaultKind::SlowConfirm(300, 1_050), 200..200 + SEEDS);
    check("slow", &runs, true);
}

#[test]
fn leaders_leave_a_site_whose_writes_fail() {
    let runs = report("writes fail 30%", FaultKind::FailWrites(300), 300..300 + SEEDS);
    check("writes", &runs, false);
}

/// B0-02's lagging leader loop (`tests/m0/results/carried-8.md`): the sick leader's clock reads whole ticks and 30% of
/// its confirms run 200 to 500 ms late, under `confirm_p99_ms`, so only LOOP_LAG moves it by STEP_DOWN.
#[test]
fn leaders_leave_a_lagging_loop() {
    let runs = report("loop lag 30%", FaultKind::LoopLag(300), 500..500 + SEEDS);
    check("lag", &runs, true);
}

#[test]
fn a_healthy_group_keeps_its_leader() {
    // No fault at all: zero leader changes and zero STEP_DOWN trips over the same minute.
    for seed in 400..400 + SEEDS {
        let mut sim = Sim::new(seed, Faults::WAN, CLIENTS, 3);
        let warm = Timestamp(sim.now.0 + WARM_MS);
        while sim.now < warm {
            sim.step(true);
        }
        let before = sim.terms.len();
        let end = Timestamp(sim.now.0 + FAULT_MS);
        while sim.now < end {
            sim.step(true);
        }
        assert_eq!(
            sim.terms.len(),
            before,
            "seed {seed}: leader changed with no fault: {:?} alarms {:?}",
            sim.terms,
            sim.alarms
        );
        assert_eq!(sim.stats.step_downs + sim.stats.holds, 0, "seed {seed}: STEP_DOWN tripped with no fault");
    }
}

/// F67: a group read only by read-index through its followers (an owner's `Authz` under API load) whose leader's site
/// goes half-hung: its AppendEntries arrive and every answer is lost. Followers shed the reads with no `suspect`
/// report, and the leader's appends keep renewing their promises. Before check quorum and the read-index failure rule
/// some seeds never elected for the whole minute (`tests/m0/results/f67.md`, paths 1 and 2). Seeds 21, 31, and 92 never
/// elected on main a7d28e5: an emptied read-index batch stayed in flight to the half-hung leader, so later reads failed
/// as queued, which forgets no leader, and no follower campaigned (F67 rule 12).
#[test]
fn a_read_only_group_elects_off_a_half_hung_leader() {
    for seed in (0..SEEDS).chain([21, 31, 92]) {
        let mut sim = Sim::new(seed, Faults::WAN, 8, 3);
        sim.edge = Edge { follower_reads: true, suspect_gap_ms: 500, reads_only: false };
        sim.run(1_500);
        let sick = leader_now(&sim).expect("leader after warm-up");
        sim.edge.reads_only = true;
        let start = sim.now;
        let until = Timestamp(start.0 + FAULT_MS);
        sim.site_faults.push(SiteFault { site: sick, from: start, until, kind: FaultKind::HalfHung(1000) });
        while sim.now < until {
            sim.step(true);
        }
        let others: Vec<SiteId> = SITES.iter().copied().filter(|s| *s != sick).collect();
        let led = sim.terms.iter().find(|t| t.2 >= start && t.1 != sick).map(|t| t.2.0 - start.0);
        // The property's T (`tests/sim.rs`, LIVE_BOUND_MS): check quorum takes ELECTION_AFTER, the followers'
        // promises another ELECTION_AFTER at most, then a campaign. The 20 s gray target is for faults the leader sees.
        assert!(led.is_some_and(|ms| ms <= 30_000), "seed {seed}: no leader off the half-hung site ({led:?} ms)");
        assert!(sim.serving(&others).is_some(), "seed {seed}: no member off the half-hung site serves");
    }
}

#[test]
#[ignore = "long liveness: RAFT_LIVE_SEEDS"]
fn long_liveness() {
    let n: u64 = std::env::var("RAFT_LIVE_SEEDS").ok().and_then(|v| v.parse().ok()).unwrap_or(100);
    let only = std::env::var("RAFT_LIVE_KIND").unwrap_or_default();
    let mut failed = Vec::new();
    if only.is_empty() || only == "calls" {
        let calls = report("calls 30%", FaultKind::Calls(300), 1_000..1_000 + n);
        failed.extend(check_all("calls", &calls, true, 1_000));
    }
    if only.is_empty() || only == "slow" {
        let slow = report("slow confirms 30% at 1 s", FaultKind::SlowConfirm(300, 1_050), 2_000..2_000 + n);
        failed.extend(check_all("slow", &slow, true, 2_000));
    }
    if only.is_empty() || only == "lag" {
        let lag = report("loop lag 30%", FaultKind::LoopLag(300), 4_000..4_000 + n);
        failed.extend(check_all("lag", &lag, true, 4_000));
    }
    if only.is_empty() || only == "writes" {
        let writes = report("writes fail 30%", FaultKind::FailWrites(300), 3_000..3_000 + n);
        failed.extend(check_all("writes", &writes, false, 3_000));
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
