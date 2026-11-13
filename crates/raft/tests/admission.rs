//! AM1-WP1's in-memory rerun of B0-29, B0-31, B0-32, and B0-33 (`build/BUILD-PLAN.md`, M0-WP9) on three real raft
//! members under admission (`graftyard_raft::admit`), over the timed network model in `common::load`. Each test
//! prints its gate quantities on one `B0-xx` line; `tests/am1/results/am1-wp1.md` quotes them.
//!
//! What is real: the members, group commit with admission, the fold's DEMOTE_PER_MIN and `hot` counts, AGENT_WAIT,
//! AGENT_QUEUE_MAX, staging and the outage refusal. What is modeled: links (one-way delay plus 0.3 ms per KB), storage
//! that confirms at once, the edge's 500 ms stage, and commands that are register writes carrying pack bytes.

mod common;

use common::load::{Done, End, Kind, Load, Op, acked_ms, pct};
use graftyard_raft::admit::{AGENT_INLINE_ENTRY, AGENT_QUEUE_MAX, AGENT_WAIT, AdmitClass, INLINE_MAX_HOT};
use graftyard_types::SiteId;

const ENAM: SiteId = SiteId(0);
const APAC: SiteId = SiteId(2);
const KIB: usize = 1 << 10;

/// Seconds of load per row: the M0 rows ran 600 s; these run long enough for every steady state the gates name.
fn secs(default: u64) -> u64 {
    std::env::var("AM1_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn op(at: u64, class: AdmitClass, actor: Option<u64>, kind: Kind, key: String, pack: usize) -> Op {
    Op { at, class, actor, kind, key, pack, expect: None, render: 0 }
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

/// B0-02's r300-3 (`tests/m0/results/carried-8.md`): a weur leader at about 300 commands/s, whose calls to apac
/// cost what that run's did (`common::load`, FAR_MS_PER_KB: a heavy tail, so calls of any size miss CALL_DEADLINE now
/// and then and answer late, and calls sent close together often arrive out of order). There apac's misses made its
/// link lossy, it got one call at a time and fell 219 entries behind, so losing enam would have stalled commits 40 to
/// 70 s [inference]. Here apac keeps up, so when enam's member stops, apac makes the quorum and the group commits
/// again within 20 s. `R300_SEEDS=a..b` runs other seeds.
#[test]
fn a_far_voter_keeps_up_so_losing_the_near_one_stalls_commits_under_20_s() {
    const WEUR: SiteId = SiteId(1);
    let s = secs(60);
    let seeds = std::env::var("R300_SEEDS").ok().and_then(|v| {
        let (a, b) = v.split_once("..")?;
        Some(a.parse::<u64>().ok()?..b.parse::<u64>().ok()?)
    });
    let mut failed = Vec::new();
    for seed in seeds.unwrap_or(12..14) {
        let mut l = Load::new(seed, WEUR);
        l.far = Some((WEUR, APAC));
        let t0 = l.now + 100;
        // 300 commands/s of about 1 KB: about 290 KB/s to each follower, as r300-3's leader wrote.
        let ops = (0..s * 300).map(|i| op(t0 + i * 10 / 3, AdmitClass::Human, None, Kind::Side, format!("w/{i}"), 900));
        l.offer(ops);
        l.run_for(s * 1000 / 2);
        let behind = l.members[WEUR.0 as usize].last_index().0 - l.members[APAC.0 as usize].last_index().0;
        let (stop, held) = (l.now, l.members[WEUR.0 as usize].last_index().0);
        l.stopped.insert(ENAM);
        l.run_for(s * 1000 / 2);
        // Until the first command in an entry appended after the stop is acked: only apac's answer commits it.
        let stall = l
            .done
            .iter()
            .filter(|d| matches!(d.end, End::Acked { index } if index > held))
            .map(|d| d.op.at + d.ms - stop)
            .min();
        let far: Vec<usize> = l.appends.iter().filter(|a| a.1 == WEUR && a.2 == APAC).map(|a| a.3).collect();
        println!(
            "r300-3 shape seed {seed}: apac {behind} entries behind at enam's stop, commits resumed {stall:?} ms after \
             it; {} calls to apac, {} missed CALL_DEADLINE, {} answered late, largest {} bytes past its first entry; \
             {} leader changes",
            far.len(),
            l.missed,
            l.late,
            far.iter().max().unwrap_or(&0),
            l.changes()
        );
        assert!(l.missed > 0 && l.late > 0, "seed {seed}: the far link never missed CALL_DEADLINE: not r300-3's");
        if !stall.is_some_and(|ms| ms <= 20_000) || l.changes() > 0 {
            failed.push((seed, stall, l.changes()));
        }
    }
    assert!(failed.is_empty(), "(seed, ms until commits resumed, leader changes): {failed:?}");
}
