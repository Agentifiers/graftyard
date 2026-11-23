//! AM1-WP1's in-memory rerun of B0-29, B0-31, B0-32, and B0-33 (`build/BUILD-PLAN.md`, M0-WP9) on three real raft
//! members under admission (`graftyard_raft::admit`), over the timed network model in `common::load`. Each test
//! prints its gate quantities on one `B0-xx` line; `tests/am1/results/am1-wp1.md` quotes them.
//!
//! What is real: the members, group commit with admission, the fold's DEMOTE_PER_MIN and `hot` counts, AGENT_WAIT,
//! AGENT_QUEUE_MAX, staging and the outage refusal. What is modeled: links (one-way delay plus 0.3 ms per KB), storage
//! that confirms at once, the edge's 500 ms stage, and commands that are register writes carrying pack bytes.

mod common;

use common::load::{Done, End, Kind, Load, Op, acked_ms, pct};
use graftyard_raft::admit::{
    AGENT_CMDS_ENTRY, AGENT_INLINE_ENTRY, AGENT_QUEUE_MAX, AGENT_WAIT, AdmitClass, INLINE_MAX_HOT,
};
use graftyard_raft::{CutEnd, ENTRY_SOFT};
use graftyard_types::SiteId;

const ENAM: SiteId = SiteId(0);
const APAC: SiteId = SiteId(2);
const KIB: usize = 1 << 10;

/// Seconds of load per row: the M0 rows ran 600 s; these run long enough for every steady state the gates name.
fn secs(default: u64) -> u64 {
    std::env::var("AM1_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn op(at: u64, class: AdmitClass, actor: Option<u64>, kind: Kind, key: String, pack: usize) -> Op {
    Op { at, class, actor, kind, key, pack, expect: None, render: 0, bound: None }
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
    l.agent = |a| a.is_some_and(|a| a >= 1_000);
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
    // AGENT_INLINE_ENTRY on every entry: at most 128 KiB of agent inline packs, past one alone.
    let worst = l.entries.iter().filter(|e| e.1 > 1).map(|e| e.3).max().unwrap_or(0);
    println!(
        "B0-29: {} 1 MiB pushes staged, {small_staged} of {} 40 KB pushes staged, largest agent inline bytes in an \
         entry {worst}",
        big.len(),
        pushes.len() - big.len()
    );
    assert!(worst <= AGENT_INLINE_ENTRY, "{worst} agent inline bytes in one entry");
}

/// ENTRY_SOFT counts no Agent inline pack bytes, which AGENT_INLINE_ENTRY bounds: on journey 9's mix the capped leader commits no slower than one that fills
/// entries to ENTRY_MAX. Counted against the cap, one 20 to 60 KB agent pack filled an entry and hosted B0-29's commit
/// p99 at 500 agents went from 702 to 1,718 ms (LM-1, `tests/am1/results/lm1-memory.md`, round 9).
#[test]
fn entry_soft_does_not_slow_journey_9() {
    let s = secs(60);
    let run = |soft: usize| {
        let mut l = Load::new(1, ENAM);
        l.agent = |a| a.is_some_and(|a| a >= 1_000);
        for m in &mut l.members {
            m.set_entry_soft(soft);
        }
        let t0 = l.now + 100;
        l.offer(journey9(t0, s, false, 29));
        l.drain(s * 1000 + 30_000);
        let mut all = acked_ms(&l.done, |_| true);
        let per = l.entries.iter().filter(|e| e.1 > 0).map(|e| e.1).sum::<usize>() as f64
            / l.entries.iter().filter(|e| e.1 > 0).count().max(1) as f64;
        (pct(&mut all, 99.0), per, l.entries.iter().map(|e| e.2).max().unwrap_or(0))
    };
    let (capped, capped_per, capped_max) = run(graftyard_raft::ENTRY_SOFT);
    let (max, max_per, _) = run(graftyard_raft::ENTRY_MAX);
    println!(
        "journey 9 {s} s: commit p99 {capped} ms at ENTRY_SOFT ({capped_per:.1} commands an entry, largest {capped_max} \
         bytes), {max} ms at ENTRY_MAX ({max_per:.1})"
    );
    assert!(capped <= max + 20, "ENTRY_SOFT commit p99 {capped} ms against {max} ms uncapped");
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
        // An agent push over INLINE_MAX_HOT but under AGENT_INLINE_ENTRY: the outage rule alone refuses it.
        ops.push(op(t + 500, AdmitClass::Agent, Some(3_000), Kind::Push, format!("refs/heads/mid/{i}"), 100 * KIB));
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
    let peak = l.samples.iter().map(|s| s.1).max().unwrap_or(0);
    let at_once = l.done.iter().filter(|d| matches!(d.end, End::Full { waited: false, .. })).count();
    println!("B0-33 capacity 300: queued agent commands peak {peak}; {at_once} refused at once at AGENT_QUEUE_MAX");
    assert!(peak <= AGENT_QUEUE_MAX.0 as usize, "{peak} agent commands queued");
    assert!(at_once > 0, "the queue cap refuses at once, before AGENT_WAIT");
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
    // Capacity is what entries of ENTRY_SOFT carry, not ENTRY_MAX: at most ENTRY_SOFT over the 200-byte command floor,
    // every 50 ms round floor, a second.
    let bound = graftyard_raft::ENTRY_SOFT as f32 / 200.0 * 1000.0 / 50.0;
    assert!(cap <= bound, "capacity {cap:.0}/s above the ENTRY_SOFT bound {bound:.0}/s");
}

/// B0-33's mix with its hosted command sizes (LM-1 round 15b, `tests/am1/results/lm1-memory.md` on `lm1-memory`, the
/// real codec over m3org's Authz fold): every side command an `IssueCall` of about 10.8 KB whose reply render, 10.3 KB,
/// no event stores, and a hosted round of 200 to 320 ms. Priced by encoded length, capacity was ENTRY_SOFT over 10.8 KB
/// a round, 30 to 47 commands/s on m26c; priced by what the log stores, the agent bucket admits B0-33's 140 line, and
/// every entry's stored bytes stay within ENTRY_SOFT past its first command.
#[test]
fn b0_33_with_hosted_render_bytes_admits_140_agent_commands_per_second() {
    const RENDER: usize = 10_300;
    let s = secs(30);
    for slow in [50, 90] {
        let mut l = Load::new(10, ENAM);
        l.slow_ms = slow;
        l.agent = |a| a.is_some_and(|a| a >= 10_000);
        let t0 = l.now + 100;
        // A side command stores about 550 bytes (538 to 635 measured): its key carries them.
        let ops = b0_33_ops(t0, s).into_iter().map(|o| match o.kind {
            Kind::Side => Op { key: format!("{}/{}", o.key, "k".repeat(500)), render: RENDER, ..o },
            _ => o,
        });
        l.offer(ops);
        l.drain(s * 1000 + 40_000);
        let m = b0_33_measure(&l, t0, s);
        let cap = l.members[0].admission().expect("leading").capacity;
        let stored = l.entries.iter().map(|e| e.2).max().unwrap_or(0);
        let round = pct(&mut l.rounds.clone(), 50.0);
        println!(
            "B0-33 with {RENDER} B renders, one-way +{slow} ms: round p50 {round} ms, capacity {cap:.0}/s, {:.1} agent \
             commands/s admitted, {} refused, Human p99 {} ms, largest entry {stored} stored bytes",
            m.agent_per_s, m.refused, m.human_p99
        );
        assert!((200..=320).contains(&round), "round p50 {round} ms, not a hosted one");
        assert!(m.agent_per_s >= 140.0, "{:.1} agent commands/s at a {round} ms round", m.agent_per_s);
        assert!(m.human_p99 <= 1_000, "Human p99 {} ms", m.human_p99);
        // Past its first command (at most a Human push of 8 KiB) an entry stores at most ENTRY_SOFT besides its agent
        // packs, which AGENT_INLINE_ENTRY bounds.
        let over: Vec<_> = l.entries.iter().filter(|e| e.2 - e.3 > graftyard_raft::ENTRY_SOFT + 9 * KIB).collect();
        assert!(over.is_empty(), "entries over ENTRY_SOFT of stored bytes: {over:?}");
    }
}

#[test]
fn a_bot_flood_cannot_take_the_agent_floor() {
    let mut l = Load::new(7, ENAM);
    l.set_capacity(Some(300.0));
    let t0 = l.now + 100;
    let mut ops = Vec::new();
    // Bot: 200 commands/s of 299,000 bytes, 60 MB/s, each over ENTRY_SOFT, so one fills an entry alone; agents:
    // 100 commands/s of 4 KB (a long key, not an inline pack), which fit only ahead of Bot. At 4 KB, ENTRY_SOFT holds
    // more than the floor's share of a round, so what the test measures is Bot against the floor, not the byte cap.
    for (i, t) in (t0..t0 + 20_000).step_by(5).enumerate() {
        ops.push(op(t, AdmitClass::Bot, None, Kind::Side, format!("bot/{i}"), 299_000));
    }
    for (i, t) in (t0..t0 + 20_000).step_by(10).enumerate() {
        ops.push(op(
            t + 1,
            AdmitClass::Agent,
            Some(i as u64 % 50),
            Kind::Side,
            format!("ag/{i}/{}", "k".repeat(4_000)),
            0,
        ));
    }
    l.offer(ops);
    l.run_for(20_500);
    // Agent commands acknowledged during the flood, after its first 5 s.
    let agent = l.done.iter().filter(|d| d.op.class == AdmitClass::Agent);
    let agent_per_s = agent
        .filter(|d| matches!(d.end, End::Acked { .. }) && (t0 + 5_000..t0 + 20_000).contains(&(d.op.at + d.ms)))
        .count() as f64
        / 15.0;
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

/// Human and MergeQueue go first in every entry: under a flood of 100 KB agent commands, which fill entries to
/// ENTRY_MAX, every Human and MergeQueue command commits in the entry after the one in flight when it arrived.
#[test]
fn humans_and_the_merge_queue_are_never_behind_agent_commands() {
    let mut l = Load::new(9, ENAM);
    // A bucket that never binds, so only lane order keeps humans ahead of the flood.
    l.set_capacity(Some(10_000.0));
    let t0 = l.now + 100;
    let mut ops = Vec::new();
    // 200 agent commands a second of 100 KB (long keys, not inline packs): 20 MB/s against 1.5 MB entries.
    for (i, t) in (t0..t0 + 20_000).step_by(5).enumerate() {
        ops.push(op(
            t,
            AdmitClass::Agent,
            Some(i as u64 % 40),
            Kind::Side,
            format!("big/{i}/{}", "k".repeat(100_000)),
            0,
        ));
    }
    for (i, t) in (t0..t0 + 20_000).step_by(250).enumerate() {
        let class = if i % 2 == 0 { AdmitClass::Human } else { AdmitClass::MergeQueue };
        // As large as the agent commands, so a cut that took agents first would leave no room for them.
        ops.push(op(t + 3, class, None, Kind::Side, format!("h/{i}/{}", "k".repeat(100_000)), 0));
    }
    l.offer(ops);
    l.run_for(21_000);
    let hm: Vec<u64> = l
        .done
        .iter()
        .filter(|d| d.op.class != AdmitClass::Agent && d.op.at >= t0)
        .map(|d| match d.end {
            End::Acked { index } => index - d.last_at_admit,
            _ => u64::MAX,
        })
        .collect();
    let mut ms = acked_ms(&l.done, |d| d.op.class != AdmitClass::Agent && d.op.at >= t0);
    println!(
        "first lanes: {} Human and MergeQueue commands, at most {} entries after admission, p99 {} ms",
        hm.len(),
        hm.iter().max().unwrap(),
        pct(&mut ms, 99.0)
    );
    assert!(hm.len() >= 75);
    assert!(hm.iter().all(|n| *n <= 1), "a Human or MergeQueue command waited behind agent commands: {hm:?}");
}

/// AGENT_WAIT: an agent command still queued 30 s after it arrived is answered `Full` and never commits.
#[test]
fn agent_wait_answers_full_after_thirty_seconds() {
    let mut l = Load::new(10, ENAM);
    // 10 agent commands a second admitted, 600 offered at once: most wait past AGENT_WAIT.
    l.set_capacity(Some(20.0));
    let t0 = l.now + 100;
    l.offer((0..600).map(|i| op(t0, AdmitClass::Agent, Some(i % 30), Kind::Side, format!("w/{i}"), 0)));
    l.drain(90_000);
    let wait = AGENT_WAIT.as_millis() as u64;
    let acked_late = l.done.iter().filter(|d| matches!(d.end, End::Acked { .. }) && d.ms > wait + 1_000).count();
    let waited = l.done.iter().filter(|d| matches!(d.end, End::Full { waited: true, .. })).collect::<Vec<_>>();
    println!(
        "AGENT_WAIT: {} acked, {} answered Full after waiting ({}..{} ms), {acked_late} acked after 31 s",
        l.done.iter().filter(|d| matches!(d.end, End::Acked { .. })).count(),
        waited.len(),
        waited.iter().map(|d| d.ms).min().unwrap_or(0),
        waited.iter().map(|d| d.ms).max().unwrap_or(0)
    );
    assert_eq!(acked_late, 0, "no agent command commits after AGENT_WAIT");
    assert!(!waited.is_empty());
    assert!(waited.iter().all(|d| d.ms >= wait && d.ms <= wait + 1_000), "answered at AGENT_WAIT");
}

/// `hot` comes from the committed log: on a group past HOT_PUSHES a 200 KiB Human push is staged (INLINE_MAX_HOT),
/// on a quiet one it stays inline, and a new leader stages it at once from what its predecessor committed.
#[test]
fn hot_is_read_from_the_log_so_a_new_leader_stages_at_once() {
    let mut l = Load::new(11, ENAM);
    let t0 = l.now + 100;
    l.offer([op(t0, AdmitClass::Human, Some(5), Kind::Push, "refs/heads/quiet".into(), 200 * KIB)]);
    l.run_for(2_000);
    let quiet = l.done.iter().find(|d| d.op.key == "refs/heads/quiet").expect("done");
    assert!(!quiet.staged, "a quiet group inlines up to INLINE_MAX");
    // 4 pushes a second for 5 s, then hand leadership to weur and push 200 KiB at once.
    let t1 = l.now;
    l.offer((0..20).map(|i| op(t1 + i * 250, AdmitClass::Human, Some(5), Kind::Push, format!("refs/heads/p{i}"), KIB)));
    l.run_for(5_100);
    l.transfer(SiteId(1));
    let mut waited = 0;
    while l.leader() != Some(SiteId(1)) && waited < 3_000 {
        l.run_for(5);
        waited += 5;
    }
    assert_eq!(l.leader(), Some(SiteId(1)));
    let t2 = l.now;
    l.offer([op(t2, AdmitClass::Human, Some(5), Kind::Push, "refs/heads/hot".into(), 200 * KIB)]);
    l.drain(5_000);
    let hot = l.done.iter().find(|d| d.op.key == "refs/heads/hot").expect("done");
    println!("hot from the log: new leader after {waited} ms; 200 KiB push staged {}", hot.staged);
    assert!(hot.staged, "the new leader read hot from the log");
}

/// `b032-shed` B: an agent command carries what is left of AGENT_PUSH_BOUND (`Admit::budget`). With the agent lane
/// backed up, one whose expected wait is past its budget is refused `Full` at once; one admitted with 2 s left is
/// answered `Full` at 2 s while still queued, not at AGENT_WAIT; Human commands beside them are never refused. And no
/// refused or expired command was acknowledged, nor committed after its answer.
#[test]
fn an_agent_command_is_answered_within_its_budget_and_never_acked_after_full() {
    let mut l = Load::new(11, ENAM);
    // 10 agent commands a second admitted: 600 offered at once fill the lane for about a minute.
    l.set_capacity(Some(20.0));
    let t0 = l.now + 100;
    let bound = |b: u64| move |o: Op| Op { bound: Some(b), ..o };
    l.offer((0..600).map(|i| op(t0, AdmitClass::Agent, Some(i % 30), Kind::Side, format!("w/{i}"), 0)));
    l.offer((0..20).map(|i| bound(2_000)(op(t0 + 50, AdmitClass::Agent, Some(900), Kind::Side, format!("b/{i}"), 0))));
    l.offer((0..20).map(|i| op(t0 + 60, AdmitClass::Human, None, Kind::Side, format!("h/{i}"), 0)));
    l.drain(90_000);
    let of = |p: &str| l.done.iter().filter(|d| d.op.key.starts_with(p)).collect::<Vec<_>>();
    let (bounded, humans) = (of("b/"), of("h/"));
    assert_eq!(bounded.len(), 20);
    // The lane held about 600 commands at 10/s: the expected wait (about 60 s) is past 2 s for every one.
    let at_once = bounded.iter().filter(|d| matches!(d.end, End::Full { waited: false, .. })).count();
    println!(
        "budget 2 s behind 600 agent commands: {at_once} of 20 refused at once, {:?}",
        bounded.iter().map(|d| d.ms).max()
    );
    assert_eq!(at_once, 20, "an agent command whose expected wait passes its budget is refused at admission");
    assert!(bounded.iter().all(|d| d.ms <= 2_000), "every bounded command answered within its bound");
    assert!(humans.iter().all(|d| matches!(d.end, End::Acked { .. })), "Human is never refused: {humans:?}");
    // The invariant: a command answered Full is never in the committed log.
    let leader = l.leader().expect("leader");
    let fold = &l.members[leader.0 as usize].group_fold().fold;
    for d in l.done.iter().filter(|d| matches!(d.end, End::Full { .. })) {
        assert!(!fold.0.contains_key(&d.op.key), "{} was answered Full and committed", d.op.key);
    }
}

/// `b032-shed` B: a bounded agent command queued behind others, whose expected wait at admission fits its budget, is
/// answered `Full` once its budget passes while it is still queued (capacity fell after it was admitted), never later,
/// and never as an ack.
#[test]
fn a_queued_agent_command_expires_at_its_budget_not_at_agent_wait() {
    let mut l = Load::new(12, ENAM);
    l.set_capacity(Some(400.0));
    let t0 = l.now + 100;
    l.offer(
        (0..30)
            .map(|i| Op { bound: Some(2_000), ..op(t0, AdmitClass::Agent, Some(i), Kind::Side, format!("q/{i}"), 0) }),
    );
    // Admitted at 200 agent commands a second; then the agent rate drops to 1 a second, so the queue stalls.
    l.run_for(105);
    l.set_capacity(Some(2.0));
    l.drain(60_000);
    let waited: Vec<_> = l.done.iter().filter(|d| matches!(d.end, End::Full { waited: true, .. })).collect();
    let acked: Vec<_> = l.done.iter().filter(|d| matches!(d.end, End::Acked { .. })).collect();
    println!(
        "{} acked, {} expired at {:?} ms",
        acked.len(),
        waited.len(),
        waited.iter().map(|d| d.ms).collect::<Vec<_>>()
    );
    assert!(!waited.is_empty(), "some queued command expired");
    assert!(waited.iter().all(|d| d.ms >= 2_000 && d.ms <= 2_100), "expired at its 2 s budget, not at AGENT_WAIT");
    let leader = l.leader().expect("leader");
    let fold = &l.members[leader.0 as usize].group_fold().fold;
    for d in &waited {
        assert!(!fold.0.contains_key(&d.op.key), "{} expired and committed", d.op.key);
    }
}

/// `b032-shed` B, the acknowledgment rule: a command appended to an entry before its budget lapses is never answered
/// `Full`. Rounds slowed to about 1.4 s, past a 1 s budget; each command arrives at an idle leader, so it is appended at
/// once, commits after its budget, and still gets its ack. A command still queued at its budget is the only kind that
/// expires (`a_queued_agent_command_expires_at_its_budget_not_at_agent_wait`).
#[test]
fn a_command_appended_before_its_budget_lapses_commits_after_it_and_is_acked() {
    let mut l = Load::new(13, ENAM);
    l.slow_ms = 600;
    let t0 = l.now + 100;
    l.offer((0..5u64).map(|i| Op {
        bound: Some(1_000),
        ..op(t0 + i * 4_000, AdmitClass::Agent, Some(i), Kind::Side, format!("a/{i}"), 0)
    }));
    l.drain(40_000);
    println!("bounded 1 s on a 1.4 s round: {:?}", l.done.iter().map(|d| (&d.end, d.ms)).collect::<Vec<_>>());
    assert_eq!(l.done.len(), 5);
    assert!(
        l.done.iter().all(|d| matches!(d.end, End::Acked { .. }) && d.ms > 1_000),
        "each was appended in time, committed past its budget, and was acknowledged"
    );
}

/// cut-trace: each cut records what ended it. Commands arriving a second apart are each cut alone with nothing left
/// (`Empty`); fifteen 10 KiB Human pushes queued behind one entry, under DEEP_QUEUE entries' worth, fill the next to
/// ENTRY_SOFT, 9 of them, since a tenth would make 102,400 bytes (`Soft`); and at a capacity of 2 commands/s the agent bucket holds one token, so
/// each cut of a queue of agent commands takes one and stops with the rest queued (`AgentTokens`).
#[test]
fn each_cut_records_what_ended_it() {
    let human =
        |at: u64, k: u64, pack: usize| op(at, AdmitClass::Human, None, Kind::Push, format!("refs/heads/h{k}"), pack);
    let mut l = Load::new(1, ENAM);
    l.cuts.clear();
    let t0 = l.now + 100;
    l.offer((0..3).map(|k| human(t0 + k * 1_000, k, 0)));
    l.drain(10_000);
    assert_eq!(l.cuts.len(), 3);
    for c in &l.cuts {
        assert_eq!((c.end, c.taken, c.left, c.in_flight), (CutEnd::Empty, [1, 0, 0, 0], [0; 4], false));
    }

    let mut l = Load::new(1, ENAM);
    l.cuts.clear();
    let t0 = l.now + 100;
    l.offer((0..16).map(|k| human(t0, k, 10 * KIB)));
    l.drain(10_000);
    // The first push is cut alone as it arrives; the other fifteen, 154 KB, queue behind its entry: not deep.
    let c = l.cuts[1];
    assert_eq!((c.end, c.taken, c.left), (CutEnd::Soft, [9, 0, 0, 0], [6, 0, 0, 0]), "{:?}", l.cuts);
    assert!(c.soft_bytes <= ENTRY_SOFT && c.soft_bytes + 10 * KIB > ENTRY_SOFT, "{c:?}");

    let mut l = Load::new(1, ENAM);
    l.set_capacity(Some(2.0));
    l.cuts.clear();
    let t0 = l.now + 100;
    let side = |k: u64| op(t0, AdmitClass::Agent, Some(1_000 + k), Kind::Side, format!("side/{k}"), 0);
    l.offer((0..6).map(side));
    l.drain(20_000);
    assert_eq!(l.done.iter().filter(|d| matches!(d.end, End::Acked { .. })).count(), 6);
    // The first is cut alone as it arrives; each later cut takes the one token a second brings.
    let (last, mid) = l.cuts[1..].split_last().expect("cuts");
    assert_eq!(mid.len(), 4, "{:?}", l.cuts);
    for (i, c) in mid.iter().enumerate() {
        assert_eq!((c.end, c.taken, c.left[3]), (CutEnd::AgentTokens, [0, 0, 0, 1], 4 - i as u32), "{c:?}");
        assert!(c.tokens < 1.0, "{c:?}");
    }
    assert_eq!((last.end, last.taken), (CutEnd::Empty, [0, 0, 0, 1]));
}

/// soft-grow: B0-35 m62's queue. With more than DEEP_QUEUE entries' worth of Human bytes waiting, a cut fills to
/// ENTRY_SOFT_DEEP, and the first cut that finds the queue shallow is back within ENTRY_SOFT. Then m62's load in
/// shape: 10 KiB Human pushes offered faster than one ENTRY_SOFT entry a round carries, for 20 s, at a hosted round.
/// At a flat ENTRY_SOFT the queue grows for the whole burst; growing, it stays within a few entries. Offered here is
/// about 1.3 times what ENTRY_SOFT entries carry at this round, as m62's minutes 7 to 9 were [inference]; at under
/// 1.5 times, ENTRY_SOFT_DEEP entries carry it.
#[test]
fn a_deep_human_queue_fills_entries_to_entry_soft_deep_and_drains() {
    use graftyard_raft::{DEEP_QUEUE, ENTRY_SOFT_DEEP};
    let human =
        |at: u64, k: u64, pack: usize| op(at, AdmitClass::Human, None, Kind::Push, format!("refs/heads/h{k}"), pack);
    let mut l = Load::new(1, ENAM);
    l.cuts.clear();
    let t0 = l.now + 100;
    l.offer((0..41).map(|k| human(t0, k, 10 * KIB)));
    l.drain(20_000);
    assert_eq!(l.done.iter().filter(|d| matches!(d.end, End::Acked { .. })).count(), 41);
    // The first push is cut alone; 40, about 420 KB, wait behind it. A cut whose queue (its batch plus what it left)
    // held more than DEEP_QUEUE entries' worth fills to ENTRY_SOFT_DEEP, with no room for one more push; any other
    // cut stays within ENTRY_SOFT.
    let cuts = &l.cuts[1..];
    assert_eq!(cuts.iter().map(|c| c.taken[0]).sum::<u32>(), 40, "{:?}", l.cuts);
    let one = cuts[0].soft_bytes / cuts[0].taken[0] as usize;
    for c in cuts {
        let queued = c.soft_bytes + c.left[0] as usize * one;
        let deep = queued > DEEP_QUEUE * ENTRY_SOFT + one;
        let bound = if deep { ENTRY_SOFT_DEEP } else { ENTRY_SOFT };
        assert!(c.soft_bytes <= bound, "a cut of {} bytes from {queued} queued: {:?}", c.soft_bytes, l.cuts);
        if deep {
            assert!(c.soft_bytes + one > bound, "a deep cut stopped early at {}: {:?}", c.soft_bytes, l.cuts);
        }
    }
    assert!(cuts[0].soft_bytes > ENTRY_SOFT, "the first cut of a deep queue did not grow: {:?}", l.cuts);
    assert!(cuts.last().is_some_and(|c| c.soft_bytes <= ENTRY_SOFT), "the bound did not drop back: {:?}", l.cuts);

    // 40 pushes a second of 10 KiB for 20 s, about 420 KB/s, against about 330 KB/s that ENTRY_SOFT entries carry at
    // this round.
    let mut l = Load::new(3, ENAM);
    l.slow_ms = 110;
    let t0 = l.now + 100;
    l.offer((0..800u64).map(|k| human(t0 + k * 25, k, 10 * KIB)));
    l.drain(80_000);
    let mut ms = acked_ms(&l.done, |_| true);
    let left_max = l.cuts.iter().map(|c| c.left[0]).max().unwrap_or(0);
    let round = pct(&mut l.rounds.clone(), 50.0);
    let (p50, p99) = (pct(&mut ms, 50.0), pct(&mut ms, 99.0));
    println!(
        "soft-grow at 40 pushes/s of 10 KiB for 20 s: {} acked, commit p50 {p50} ms p99 {p99} ms, at most {left_max} \
         queued after a cut, round p50 {round} ms, {} leader changes",
        ms.len(),
        l.changes()
    );
    assert_eq!(ms.len(), 800);
    assert_eq!(l.changes(), 0, "{:?}", l.leaders);
    assert!((250..=450).contains(&round), "round p50 {round} ms, not a hosted one");
    // Main's flat cut on this load: commit p50 6,090 ms, p99 11,470 ms, 286 queued; this rule: 1,205, 1,800 and 44.
    assert!(left_max <= 80, "the queue grew to {left_max} pushes");
    assert!(p99 <= 3_000, "commit p99 {p99} ms");
}

/// agent-cmds-entry: B0-32 m3fgt1's burst put 307 staged agent pushes in the leader's queue at once, and its cuts took
/// 191 and 160 of them, whose entries took 2.6 and 2.8 s. With the cap, no cut takes more than AGENT_CMDS_ENTRY Agent
/// commands; a cut it ended says `AgentCmds`; the Human push queued with them still goes in the first cut after it
/// arrives; and every agent push commits. Capacity is fixed high so the token bucket, which never bound on m3fgt1, does
/// not bind here either.
#[test]
fn a_burst_of_agent_commands_is_cut_at_agent_cmds_entry_per_entry() {
    let mut l = Load::new(1, ENAM);
    l.set_capacity(Some(2_000.0));
    l.cuts.clear();
    let t0 = l.now + 100;
    let mut ops: Vec<Op> = (0..307u64)
        .map(|k| op(t0 + k % 5, AdmitClass::Agent, Some(1_000 + k), Kind::Push, format!("refs/heads/b/{k}"), 0))
        .collect();
    ops.push(op(t0 + 3, AdmitClass::Human, None, Kind::Push, "refs/heads/human".into(), 0));
    l.offer(ops);
    l.drain(60_000);
    let agent = l.cuts.iter().map(|c| c.taken[3] as usize).collect::<Vec<_>>();
    let most = agent.iter().copied().max().unwrap_or(0);
    println!("agent-cmds-entry: {} cuts, agent commands per cut {agent:?}", l.cuts.len());
    assert_eq!((AGENT_CMDS_ENTRY, most), (64, 64), "the largest cut takes the cap, no more: {agent:?}");
    // The first push is cut alone as it arrives; the other 306 queue behind its entry and leave 64 at a time.
    assert_eq!(agent, [1, 64, 64, 64, 64, 50], "every agent push is cut once, 64 to an entry");
    assert!(l.cuts.iter().any(|c| c.end == CutEnd::AgentCmds), "{:?}", l.cuts);
    let human = l.cuts.iter().position(|c| c.taken[0] == 1).expect("the human push is cut");
    assert!(human <= 1, "the human push waits behind no agent cut: cut {human} of {:?}", l.cuts);
    let pushes = of(&l.done, Kind::Push);
    assert_eq!(pushes.iter().filter(|d| matches!(d.end, End::Acked { .. })).count(), 308, "every push commits");
}

/// edge-refusal: a slow leader bound by AGENT_CMDS_ENTRY, not by its bucket. Rounds stretched to a slow host's
/// (`slow_ms`) leave about 64 agent commands an entry, while the bucket sized from bytes (`capacity`) reads several
/// times that, as B0-33's pfA and pgA did (bucket about 1.3x the drain, `Trace::Cut`). 200 bounded agent commands a
/// second are offered for 30 s. Sized from the bucket, the leader admits commands whose real wait passes their 5 s
/// bound, and they expire in the lane; sized from its measured drain, it refuses them at once instead, so nearly none
/// expire, and the agents it does admit still get at least the drain. Humans commit throughout.
#[test]
fn a_slow_leader_refuses_at_once_what_it_cannot_drain_within_the_bound() {
    let mut l = Load::new(21, ENAM);
    l.slow_ms = 300;
    let t0 = l.now + 100;
    let s = 30u64;
    let mut ops = Vec::new();
    for (i, t) in (t0..t0 + s * 1000).step_by(5).enumerate() {
        let i = i as u64;
        let o = op(t, AdmitClass::Agent, Some(1_000 + i % 2_000), Kind::Side, format!("s/{i}"), 0);
        ops.push(Op { bound: Some(5_000), ..o });
    }
    for (i, t) in (t0..t0 + s * 1000).step_by(500).enumerate() {
        ops.push(op(t + 2, AdmitClass::Human, Some(7), Kind::Side, format!("h/{i}"), 0));
    }
    l.offer(ops);
    l.drain(s * 1000 + 40_000);
    let agent = |d: &&Done| d.op.class == AdmitClass::Agent;
    let expired = l.done.iter().filter(agent).filter(|d| matches!(d.end, End::Full { waited: true, .. })).count();
    let at_once = l.done.iter().filter(agent).filter(|d| matches!(d.end, End::Full { waited: false, .. })).count();
    let acked = l.done.iter().filter(agent).filter(|d| matches!(d.end, End::Acked { .. })).count();
    let late = l.done.iter().filter(agent).filter(|d| matches!(d.end, End::Acked { .. }) && d.ms > 5_000).count();
    let cut: u32 = l.cuts.iter().map(|c| c.taken[3]).sum();
    let mut h = acked_ms(&l.done, |d| d.op.class == AdmitClass::Human);
    let v = l.members[l.leader().unwrap().0 as usize].admission().unwrap();
    println!(
        "edge-refusal slow leader: {acked} acked ({late} past 5 s), {at_once} refused at once, {expired} expired; \
         {cut} agent commands cut; capacity {:.0}/s, agent rate {:.0}/s; human p99 {} ms",
        v.capacity,
        v.agent_rate,
        pct(&mut h, 99.0)
    );
    let offered = l.done.iter().filter(agent).count();
    assert_eq!(offered, 6_000);
    assert!(at_once > 1_000, "the leader refused what it could not drain at once: {at_once}");
    assert!(expired * 50 < offered, "at most 2% of agent commands expired in the lane, not {expired}");
    // Not starved: the leader kept draining at its rate through the window.
    assert!(acked as f64 / s as f64 >= 40.0, "agents still committed {acked} in {s} s");
    assert!(h.iter().all(|&ms| ms < 5_000), "Human never waits behind the agent lane");
    assert!(l.done.iter().filter(|d| d.op.class == AdmitClass::Human).all(|d| matches!(d.end, End::Acked { .. })));
}

/// edge-refusal: the measured drain is never taken above the AGENT_SHARE bucket's rate. At a burst's start the
/// leader holds a full bucket (one second of tokens), so its cuts take AGENT_CMDS_ENTRY agent commands each round,
/// faster than the bucket will refill, until the tokens run out; an advert sized from that drain would read a wait
/// the bucket then breaks.
#[test]
fn the_advertised_agent_rate_never_passes_the_bucket() {
    let mut l = Load::new(22, ENAM);
    l.set_capacity(Some(600.0));
    let t0 = l.now + 100;
    l.offer((0..3_000u64).map(|i| op(t0 + i % 3, AdmitClass::Agent, Some(i % 40), Kind::Side, format!("c/{i}"), 0)));
    let mut most = 0.0f32;
    for _ in 0..40 {
        l.run_for(100);
        if let Some(v) = l.leader().and_then(|s| l.members[s.0 as usize].admission()) {
            most = most.max(v.agent_rate);
        }
    }
    println!("edge-refusal burst at capacity 600: highest advertised agent rate {most:.1}/s");
    assert!(most > 0.0);
    assert!(most <= 300.0 + 1e-3, "advertised {most} agent commands a second against a bucket of 300");
}
