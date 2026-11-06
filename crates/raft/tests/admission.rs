//! AM1-WP1's in-memory rerun of B0-29, B0-31, B0-32, and B0-33 (`build/BUILD-PLAN.md`, M0-WP9) on three real raft
//! members under admission (`graftyard_raft::admit`), over the timed network model in `common::load`. Each test
//! prints its gate quantities on one `B0-xx` line; `tests/am1/results/am1-wp1.md` quotes them.
//!
//! What is real: the members, group commit with admission, the fold's DEMOTE_PER_MIN and `hot` counts, AGENT_WAIT,
//! AGENT_QUEUE_MAX, staging and the outage refusal. What is modeled: links (one-way delay plus 0.3 ms per KB), storage
//! that confirms at once, the edge's 500 ms stage, and commands that are register writes carrying pack bytes.

mod common;

use common::load::{Done, End, Kind, Load, Op, acked_ms, pct};
use graftyard_raft::admit::{AGENT_QUEUE_MAX, AdmitClass, INLINE_MAX_HOT};
use graftyard_types::SiteId;

const ENAM: SiteId = SiteId(0);
const APAC: SiteId = SiteId(2);
const KIB: usize = 1 << 10;

/// Seconds of load per row: the M0 rows ran 600 s; these run long enough for every steady state the gates name.
fn secs(default: u64) -> u64 {
    std::env::var("AM1_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn op(at: u64, class: AdmitClass, actor: Option<u64>, kind: Kind, key: String, pack: usize) -> Op {
    Op { at, class, actor, kind, key, pack, expect: None }
}

/// Journey 9's mix from `t0` for `secs`: 500 agents push distinct refs every 2 min (4.17/s; 40 KB, 5% at 1 MiB),
/// side commands to 27.6 commands/s in all (PrSync, test-merge records, check runs, task updates, lease renewals,
/// classed Agent as their causes are), and 10 queue merges a minute. `big` sets every agent pack to 1 MiB (B0-31).
fn journey9(t0: u64, secs: u64, big: bool, seed: u64) -> Vec<Op> {
    let mut rng = common::sim::Rng::new(seed);
    let mut ops = Vec::new();
    let end = t0 + secs * 1000;
    let mut k = 0u64;
    let mut t = t0;
    while t < end {
        // One of 27.6 commands/s: 4.17 pushes, 0.17 merges, the rest side commands.
        let r = rng.below(276);
        let pick = if r < 42 {
            let pack = if big || rng.chance(50) { 1 << 20 } else { 20 * KIB + rng.below(40 * KIB as u64) as usize };
            op(t, AdmitClass::Agent, Some(1_000 + k % 500), Kind::Push, format!("refs/heads/agent/{k}"), pack)
        } else if r < 44 {
            op(t, AdmitClass::MergeQueue, None, Kind::Merge, format!("refs/heads/main#{k}"), 4 * KIB)
        } else {
            op(t, AdmitClass::Agent, Some(1_000 + k % 500), Kind::Side, format!("side/{}", k % 2_000), 0)
        };
        ops.push(pick);
        k += 1;
        t += 36;
    }
    ops
}

fn of(done: &[Done], kind: Kind) -> Vec<&Done> {
    done.iter().filter(|d| d.op.kind == kind).collect()
}

#[test]
fn b0_29_the_journey_9_mix_commits_under_600_ms_with_no_stale_and_merges_in_the_next_entry() {
    let s = secs(60);
    let mut l = Load::new(1, ENAM);
    let t0 = l.now + 100;
    l.offer(journey9(t0, s, false, 29));
    l.drain(s * 1000 + 30_000);
    let mut all = acked_ms(&l.done, |_| true);
    let mut merges = acked_ms(&l.done, |d| d.op.kind == Kind::Merge);
    let stale = l.done.iter().filter(|d| d.end == End::Stale).count();
    let refused = l.done.iter().filter(|d| matches!(d.end, End::Full { .. })).count();
    let staged = l.done.iter().filter(|d| d.staged).count();
    let (p50, p99, m99) = (pct(&mut all, 50.0), pct(&mut all, 99.0), pct(&mut merges, 99.0));
    println!(
        "B0-29 {s} s: {} commands, commit p50 {p50} ms p99 {p99} ms, merge p99 {m99} ms, {stale} Stale, {refused} \
         refused, {staged} staged, {} leader changes",
        l.done.len(),
        l.changes()
    );
    assert!(p99 <= 600, "commit p99 {p99} ms");
    assert!(m99 <= 2_000, "merge p99 {m99} ms");
    assert_eq!(stale, 0, "distinct refs never go Stale");
    assert_eq!(refused, 0, "27.6 commands/s is under every cap");
    assert_eq!(l.changes(), 0, "no leader change: {:?}", l.leaders);
    // The 1 MiB pushes ride a hot group (4.17 pushes/s over HOT_PUSHES), or pass AGENT_INLINE_ENTRY, so all are
    // staged; a 40 KB one is staged only when queued agent inline bytes would pass the budget.
    let pushes = of(&l.done, Kind::Push);
    let big: Vec<&&Done> = pushes.iter().filter(|d| d.op.pack > INLINE_MAX_HOT).collect();
    assert!(!big.is_empty() && big.iter().all(|d| d.staged), "every 1 MiB agent push is staged");
    let small_staged = pushes.iter().filter(|d| d.op.pack <= INLINE_MAX_HOT && d.staged).count();
    println!(
        "B0-29: {} 1 MiB pushes staged, {small_staged} of {} 40 KB pushes staged",
        big.len(),
        pushes.len() - big.len()
    );
}

#[test]
fn b0_31_an_apac_leader_stages_1_mib_agent_pushes_and_refuses_them_at_once_while_placement_fails() {
    let s = secs(30);
    // Buckets up: every agent push at 1 MiB on a hot group is staged, and commit p99 holds at 1 s.
    let mut l = Load::new(2, APAC);
    let t0 = l.now + 100;
    l.offer(journey9(t0, s, true, 31));
    l.drain(s * 1000 + 30_000);
    let mut up = acked_ms(&l.done, |_| true);
    let pushes = of(&l.done, Kind::Push);
    // Before the group is hot the per-entry budget stages them too: an agent pack of 1 MiB is past AGENT_INLINE_ENTRY.
    let staged = pushes.iter().filter(|d| d.staged).count();
    let p99_up = pct(&mut up, 99.0);
    println!(
        "B0-31 buckets up {s} s: commit p99 {p99_up} ms, {staged} of {} agent pushes staged, {} leader changes",
        pushes.len(),
        l.changes()
    );
    assert!(p99_up <= 1_000, "commit p99 {p99_up} ms with buckets up");
    assert_eq!(staged, pushes.len(), "every 1 MiB agent push is staged");

    // Placement failing: agent pushes over INLINE_MAX_HOT are refused at once; Human and MergeQueue pushes up to 1
    // MiB stay inline and none is refused.
    let mut l = Load::new(3, APAC);
    l.placement_ok = false;
    let t0 = l.now + 100;
    let mut ops = journey9(t0, s, true, 32);
    for (i, t) in (t0..t0 + s * 1000).step_by(2_000).enumerate() {
        ops.push(op(t, AdmitClass::Human, Some(7), Kind::Push, format!("refs/heads/human/{i}"), 1 << 20));
        ops.push(op(t + 1_000, AdmitClass::MergeQueue, None, Kind::Merge, format!("refs/heads/mq/{i}"), 1 << 20));
    }
    l.offer(ops);
    l.drain(s * 1000 + 30_000);
    let hm: Vec<&Done> =
        l.done.iter().filter(|d| matches!(d.op.class, AdmitClass::Human | AdmitClass::MergeQueue)).collect();
    let hm_refused = hm.iter().filter(|d| !matches!(d.end, End::Acked { .. })).count();
    let mut hm_ms = acked_ms(&l.done, |d| matches!(d.op.class, AdmitClass::Human | AdmitClass::MergeQueue));
    let agent_pushes =
        of(&l.done, Kind::Push).into_iter().filter(|d| d.op.class == AdmitClass::Agent).collect::<Vec<_>>();
    let at_once = agent_pushes
        .iter()
        .filter(|d| matches!(d.end, End::Full { waited: false, retry_after_s } if retry_after_s >= 1) && d.ms == 0)
        .count();
    let p99_down = pct(&mut hm_ms, 99.0);
    println!(
        "B0-31 placement failing {s} s: Human and MergeQueue p99 {p99_down} ms, {hm_refused} of {} refused; {at_once} \
         of {} agent pushes refused at once; {} leader changes",
        hm.len(),
        agent_pushes.len(),
        l.changes()
    );
    assert!(p99_down <= 3_000, "Human and MergeQueue p99 {p99_down} ms");
    assert_eq!(hm_refused, 0, "no Human or MergeQueue push up to 1 MiB is refused");
    assert_eq!(at_once, agent_pushes.len(), "every 1 MiB agent push is refused at once in the `ng` shape");
}

#[test]
fn b0_32_a_burst_of_500_agent_pushes_in_one_second_on_a_hot_group_all_commit_and_merges_wait_at_most_two_entries() {
    let mut l = Load::new(4, ENAM);
    // Make the group hot first: 3 pushes a second for 6 s.
    let t0 = l.now + 100;
    let warm: Vec<Op> = (0..18)
        .map(|i| op(t0 + i * 333, AdmitClass::Human, Some(9), Kind::Push, format!("refs/heads/warm/{i}"), 8 * KIB))
        .collect();
    l.offer(warm);
    l.run_for(6_500);
    let b0 = l.now;
    let mut ops: Vec<Op> = (0..500)
        .map(|i| {
            op(b0 + i * 2, AdmitClass::Agent, Some(i % 500), Kind::Push, format!("refs/heads/burst/{i}"), 256 * KIB)
        })
        .collect();
    for i in 0..5 {
        ops.push(op(
            b0 + 100 + i * 200,
            AdmitClass::MergeQueue,
            None,
            Kind::Merge,
            format!("refs/heads/m{i}"),
            4 * KIB,
        ));
    }
    l.offer(ops);
    l.drain(40_000);
    let burst: Vec<&Done> = l.done.iter().filter(|d| d.op.key.starts_with("refs/heads/burst/")).collect();
    let in5 = burst.iter().filter(|d| matches!(d.end, End::Acked { .. }) && d.ms <= 5_000).count();
    let refused = burst.iter().filter(|d| matches!(d.end, End::Full { .. })).count();
    let late = burst.iter().filter(|d| matches!(d.end, End::Acked { .. }) && d.ms > 5_000).count();
    let mut ms: Vec<u64> = burst.iter().map(|d| d.ms).collect();
    let merges: Vec<&Done> = l.done.iter().filter(|d| d.op.kind == Kind::Merge && d.op.at >= b0).collect();
    let merge_entries: Vec<u64> = merges
        .iter()
        .map(|d| match d.end {
            End::Acked { index } => index - d.last_at_admit,
            _ => u64::MAX,
        })
        .collect();
    let merge_ms: Vec<u64> = merges.iter().map(|d| d.ms).collect();
    let peak_bytes = l.samples.iter().map(|s| s.2).max().unwrap_or(0);
    println!(
        "B0-32: {in5} of {} burst pushes acked within 5 s, {refused} refused, {late} late, client p99 {} ms; merges \
         {merge_ms:?} ms, entries after admission {merge_entries:?}; queued agent bytes peak {peak_bytes}; {} leader \
         changes",
        burst.len(),
        pct(&mut ms, 99.0),
        l.changes()
    );
    assert_eq!(burst.len(), 500);
    assert_eq!(late, 0, "every push within 5 s or refused");
    assert_eq!(in5 + refused, 500);
    assert!(peak_bytes < AGENT_QUEUE_MAX.1);
    assert!(merge_entries.iter().all(|n| *n <= 2), "every merge in one of the next 2 entries: {merge_entries:?}");
    assert!(merge_ms.iter().all(|ms| *ms <= 2_000), "no merge delayed over 2 s: {merge_ms:?}");
    assert_eq!(l.changes(), 0, "{:?}", l.leaders);
}

/// B0-33's offered load from `t0` for `secs`: 5,000 agents at 252 agent-class commands/s (pushes of 8 KiB and side
/// commands), one user token at 100 commands a minute classed Human by the edge, Human pushes at 1/s and MergeQueue
/// merges at 10 a minute.
fn b0_33_ops(t0: u64, secs: u64) -> Vec<Op> {
    let mut ops = Vec::new();
    let end = t0 + secs * 1000;
    for (i, t) in (t0..end).step_by(4).enumerate() {
        // 250/s, plus 2/s below: 252.
        let i = i as u64;
        if i.is_multiple_of(10) {
            ops.push(op(
                t,
                AdmitClass::Agent,
                Some(10_000 + i % 5_000),
                Kind::Push,
                format!("refs/heads/a/{i}"),
                8 * KIB,
            ));
        } else {
            ops.push(op(t, AdmitClass::Agent, Some(10_000 + i % 5_000), Kind::Side, format!("s/{i}"), 0));
        }
    }
    for (i, t) in (t0..end).step_by(500).enumerate() {
        ops.push(op(t + 1, AdmitClass::Agent, Some(20_000 + i as u64), Kind::Side, format!("x/{i}"), 0));
    }
    for (i, t) in (t0..end).step_by(600).enumerate() {
        ops.push(op(t + 2, AdmitClass::Human, Some(42), Kind::Side, format!("user/{i}"), 0));
    }
    for (i, t) in (t0..end).step_by(1_000).enumerate() {
        ops.push(op(t + 3, AdmitClass::Human, Some(7), Kind::Push, format!("refs/heads/h/{i}"), 8 * KIB));
    }
    for (i, t) in (t0..end).step_by(6_000).enumerate() {
        ops.push(op(t + 5, AdmitClass::MergeQueue, None, Kind::Merge, format!("refs/heads/mq/{i}"), 4 * KIB));
    }
    ops
}

/// The B0-33 gate quantities of a finished run from `t0`.
struct B033 {
    agent_per_s: f64,
    human_p99: u64,
    mq_p99: u64,
    mq_max: u64,
    user_as_agent: usize,
    user_after_60: usize,
    refused: usize,
    refusals_bad: usize,
}

fn b0_33_measure(l: &Load, t0: u64, secs: u64) -> B033 {
    // Agent commands acknowledged inside the load window, by when they were acknowledged, after the first 5 s.
    let (from, to) = (t0 + 5_000, t0 + secs * 1000);
    let w = (secs - 5) as f64;
    let agent = l.done.iter().filter(|d| d.op.class == AdmitClass::Agent && d.op.actor != Some(42));
    let agent_per_s =
        agent.filter(|d| matches!(d.end, End::Acked { .. }) && (from..to).contains(&(d.op.at + d.ms))).count() as f64
            / w;
    let mut h = acked_ms(&l.done, |d| d.op.class == AdmitClass::Human && d.op.actor == Some(7));
    let mut mq = acked_ms(&l.done, |d| d.op.class == AdmitClass::MergeQueue);
    let user: Vec<&Done> = l.done.iter().filter(|d| d.op.actor == Some(42)).collect();
    let mut by_minute = user.clone();
    by_minute.sort_by_key(|d| d.op.at);
    let user_after_60 = by_minute.iter().skip(60).count();
    // Classed Agent: admitted as Agent, or refused by agent admission, which only an Agent command can be.
    let user_as_agent = by_minute
        .iter()
        .skip(60)
        .filter(|d| d.admitted == Some(AdmitClass::Agent) || matches!(d.end, End::Full { .. }))
        .count();
    let refusals: Vec<&Done> = l.done.iter().filter(|d| matches!(d.end, End::Full { .. })).collect();
    let refusals_bad = refusals
        .iter()
        .filter(|d| d.op.class != AdmitClass::Agent && d.admitted != Some(AdmitClass::Agent))
        .filter(|d| !matches!(d.end, End::Full { retry_after_s, .. } if retry_after_s >= 1))
        .count();
    B033 {
        agent_per_s,
        human_p99: pct(&mut h, 99.0),
        mq_p99: pct(&mut mq.clone(), 99.0),
        mq_max: pct(&mut mq, 100.0),
        user_as_agent,
        user_after_60,
        refused: refusals.len(),
        refusals_bad,
    }
}

#[test]
fn b0_33_at_a_pinned_300_commands_per_second_agents_get_their_share_and_humans_keep_their_latency() {
    let s = secs(60);
    let mut l = Load::new(5, ENAM);
    // As M0's `cap300r1`: capacity pinned at 300, so the agent bucket (150/s) binds.
    l.set_capacity(Some(300.0));
    let t0 = l.now + 100;
    l.offer(b0_33_ops(t0, s));
    l.drain(s * 1000 + 40_000);
    let m = b0_33_measure(&l, t0, s);
    let human_refused = l.done.iter().filter(|d| d.op.actor == Some(7) && !matches!(d.end, End::Acked { .. })).count();
    println!(
        "B0-33 capacity 300, {s} s: {:.1} agent commands/s admitted, {} refused (AGENT_QUEUE_MAX or AGENT_WAIT), \
         Human p99 {} ms, MergeQueue p99 {} ms (max {} ms), user token {} of {} writes after its 60th classed Agent, \
         {} leader changes",
        m.agent_per_s,
        m.refused,
        m.human_p99,
        m.mq_p99,
        m.mq_max,
        m.user_as_agent,
        m.user_after_60,
        l.changes()
    );
    assert!(m.agent_per_s >= 140.0, "{:.1} agent commands/s", m.agent_per_s);
    assert!(m.agent_per_s <= 160.0, "the bucket holds agents to AGENT_SHARE: {:.1}/s", m.agent_per_s);
    assert!(m.human_p99 <= 1_000 && m.mq_p99 <= 1_000, "Human {} ms, MergeQueue {} ms", m.human_p99, m.mq_p99);
    assert!(m.mq_max <= 2_000);
    assert_eq!(human_refused, 0);
    assert!(m.refused > 0, "252/s against 150/s refuses some");
    assert_eq!(m.refusals_bad, 0, "every refusal is an agent's, with a retry hint");
    assert!(
        m.user_after_60 > 0 && m.user_as_agent * 10 >= m.user_after_60 * 9,
        "{} of {}",
        m.user_as_agent,
        m.user_after_60
    );
    assert_eq!(l.changes(), 0, "{:?}", l.leaders);
}

#[test]
fn b0_33_at_measured_capacity_every_agent_command_is_admitted() {
    let s = secs(30);
    let mut l = Load::new(6, ENAM);
    let t0 = l.now + 100;
    l.offer(b0_33_ops(t0, s));
    l.drain(s * 1000 + 40_000);
    let m = b0_33_measure(&l, t0, s);
    let cap = l.members[0].admission().map(|v| v.capacity).unwrap_or(0.0);
    println!(
        "B0-33 measured capacity {cap:.0}/s, {s} s: {:.1} agent commands/s admitted, {} refused, Human p99 {} ms, \
         MergeQueue p99 {} ms, user token {} of {} classed Agent",
        m.agent_per_s, m.refused, m.human_p99, m.mq_p99, m.user_as_agent, m.user_after_60
    );
    assert!(m.agent_per_s >= 240.0, "{:.1}/s", m.agent_per_s);
    assert_eq!(m.refused, 0);
    assert!(m.human_p99 <= 1_000 && m.mq_p99 <= 1_000);
}

#[test]
fn a_bot_flood_cannot_take_the_agent_floor() {
    let mut l = Load::new(7, ENAM);
    l.set_capacity(Some(300.0));
    let t0 = l.now + 100;
    let mut ops = Vec::new();
    // Bot: 60 commands/s of 100 KB, 6 MB/s, which fills every 1.5 MB entry; agents: 100 commands/s.
    for (i, t) in (t0..t0 + 20_000).step_by(16).enumerate() {
        ops.push(op(t, AdmitClass::Bot, None, Kind::Side, format!("bot/{i}"), 100 * KIB));
    }
    for (i, t) in (t0..t0 + 20_000).step_by(10).enumerate() {
        ops.push(op(t + 1, AdmitClass::Agent, Some(i as u64 % 50), Kind::Side, format!("ag/{i}"), 0));
    }
    l.offer(ops);
    l.drain(60_000);
    let agent = l.done.iter().filter(|d| d.op.class == AdmitClass::Agent && d.op.at >= t0 + 5_000);
    let agent_per_s = agent.filter(|d| matches!(d.end, End::Acked { .. })).count() as f64 / 15.0;
    println!("AGENT_FLOOR: {agent_per_s:.1} agent commands/s admitted under a Bot flood at capacity 300");
    // AGENT_FLOOR of 300 is 60 a second, which Bot cannot take.
    assert!(agent_per_s >= 50.0, "{agent_per_s:.1}/s");
}

#[test]
fn demotion_counts_live_in_the_fold_so_a_new_leader_demotes_at_once() {
    let mut l = Load::new(8, ENAM);
    let t0 = l.now + 100;
    // 100 writes a minute from one user token, classed Human by the edge.
    let ops: Vec<Op> =
        (0..200).map(|i| op(t0 + i * 600, AdmitClass::Human, Some(42), Kind::Side, format!("u/{i}"), 0)).collect();
    l.offer(ops);
    l.run_for(60_000);
    // A transfer away from home would come back by the home-return rule; here a STEP_DOWN-style move to weur is
    // enough, and the test reads only the first minute after it.
    l.transfer(SiteId(1));
    l.drain(120_000);
    let changed = l.leaders.get(1).map(|x| x.0).expect("weur took over");
    assert_eq!(l.leaders[1].1, SiteId(1));
    // Every write admitted by the new leader in its first minute was classed Agent: it read the count from the fold.
    let after: Vec<&Done> = l.done.iter().filter(|d| d.op.at >= changed && d.op.at < changed + 60_000).collect();
    let as_human = after.iter().filter(|d| d.admitted == Some(AdmitClass::Human)).count();
    println!(
        "DEMOTE_PER_MIN: leader change at {} ms; {as_human} of {} writes in the next minute classed Human",
        changed - t0,
        after.len()
    );
    assert!(!after.is_empty());
    assert_eq!(as_human, 0, "the new leader classed the fast token Human again");
}
