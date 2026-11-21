//! Many seeds of the simulator with every invariant checked after each step, then the history's linearizability.
//!
//! `cargo test -p graftyard-raft` runs `SEEDS` seeds. `RAFT_SEEDS=n RAFT_STEPS=m cargo test -p graftyard-raft --release
//! --test sim -- --ignored --nocapture` runs the long sweep, and `RAFT_HISTORY=path` also writes every seed's history
//! for `tests/m0/porcupine` (the gray half to `path.gray`, the live half, at least `LIVE_SEEDS_MIN` seeds, to
//! `path.live`).

mod common;

use common::history::{self, HOp, HOut, Row, Verdict};
use common::sim::{Edge, Faults, Sim};

const SEEDS: u64 = 60;
const STEPS: u64 = 8_000;

/// The liveness bound T (F67, `tests/m0/results/f67.md`): while the same two or more voters stay up, linked, and free
/// of gray faults, and a client request waits, some request completes within T. A transfer onto a bad site holds
/// commits for one LEASE, the waivers of RESET_QUARANTINE and the drain bar take 2 x ELECTION_AFTER, and a campaign
/// and a round finish it: 9 + 20 + 1 s.
const LIVE_BOUND_MS: u64 = 30_000;

/// The F67 mixes: half-hung sites (answers lost), host transfers to a named target, drains, quick resets, and the
/// hosted edge (`Edge`): follower reads that shed with no report, damped `suspect`, and on half the seeds reads only.
fn live_faults(seed: u64) -> (Faults, Edge) {
    let base = Faults { max_delay_ms: 150, gray: 3, half_hung: true, transfer: 3, ..Faults::NONE };
    let faults = match seed % 4 {
        0 => base,
        1 => Faults { drain: 1, ..base },
        2 => Faults { crash: 2, down_max_ms: 200, ..base },
        _ => Faults { drain: 1, crash: 2, down_max_ms: 500, drop: 20, partition: 1, crash_mid: 1, ..base },
    };
    (faults, Edge { follower_reads: true, suspect_gap_ms: 1_000, reads_only: seed % 8 >= 4 })
}

fn env(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

struct Totals {
    seeds: u64,
    steps: u64,
    ops: usize,
    acked: u64,
    refused: u64,
    reads: u64,
    gave_up: u64,
    crashes: u64,
    partitions: u64,
    dropped: u64,
    duplicated: u64,
    messages: u64,
    leaders: u64,
    replays: u64,
    crashes_mid: u64,
    hedges: u64,
    missed: u64,
    suspects: u64,
    forwarded: u64,
    call_failures: u64,
    step_downs: u64,
    holds: u64,
    gray: u64,
    write_failures: u64,
    early_lost: u64,
    ghosts: u64,
    id_blocks: u64,
    id_blocks_committed: u64,
    records: u64,
    records_committed: u64,
    compacted: u64,
    catchups: u64,
    early_asks: u64,
    deps_entries: u64,
    rereads: u64,
    joins: u64,
    pipelined: u64,
    late_appends: u64,
    /// The liveness monitor's longest stretch (F67): ms, seed, and when it began.
    stuck: (u64, u64, u64),
}

fn sweep(seeds: std::ops::Range<u64>, steps: u64, faults: Faults) -> Totals {
    sweep_with(seeds, steps, |_| (faults, Edge::default()))
}

fn sweep_with(seeds: std::ops::Range<u64>, steps: u64, mix: impl Fn(u64) -> (Faults, Edge)) -> Totals {
    let faults = mix(seeds.start).0;
    let mut t = Totals {
        seeds: 0,
        steps: 0,
        ops: 0,
        acked: 0,
        refused: 0,
        reads: 0,
        gave_up: 0,
        crashes: 0,
        partitions: 0,
        dropped: 0,
        duplicated: 0,
        messages: 0,
        leaders: 0,
        replays: 0,
        crashes_mid: 0,
        hedges: 0,
        missed: 0,
        suspects: 0,
        forwarded: 0,
        call_failures: 0,
        step_downs: 0,
        holds: 0,
        gray: 0,
        write_failures: 0,
        early_lost: 0,
        ghosts: 0,
        id_blocks: 0,
        id_blocks_committed: 0,
        records: 0,
        records_committed: 0,
        compacted: 0,
        catchups: 0,
        early_asks: 0,
        deps_entries: 0,
        rereads: 0,
        joins: 0,
        pipelined: 0,
        late_appends: 0,
        stuck: (0, 0, 0),
    };
    // The gray half of the long sweep writes next to the HARSH half's file, as `<path>.gray`.
    let suffix = if faults.half_hung {
        ".live"
    } else if faults.gray > 0 {
        ".gray"
    } else {
        ""
    };
    let out = std::env::var("RAFT_HISTORY").ok().map(|p| format!("{p}{suffix}"));
    let mut file = String::new();
    for seed in seeds {
        let (faults, edge) = mix(seed);
        let mut sim = Sim::new(seed, faults, 4, 3);
        sim.edge = edge;
        sim.run(steps);
        let (stuck, at) = sim.longest_leaderless();
        if stuck > t.stuck.0 {
            t.stuck = (stuck, seed, at.0);
        }
        assert!(
            stuck <= LIVE_BOUND_MS,
            "seed {seed}: from {} ms a healthy quorum had requests waiting and none completed for {stuck} ms",
            at.0
        );
        sim.settle(4_000);
        assert_eq!(history::check(&sim.history), Verdict::Ok, "seed {seed}: history is not linearizable");
        if out.is_some() {
            file.push_str(&history::jsonl(&sim.history, &format!("s{seed}/"), (seed as usize) * 16));
        }
        let s = sim.stats;
        t.seeds += 1;
        t.steps += s.steps;
        t.ops += sim.history.len();
        t.acked += s.acked;
        t.refused += s.refused;
        t.reads += s.reads;
        t.gave_up += s.gave_up;
        t.crashes += s.crashes;
        t.partitions += s.partitions;
        t.dropped += s.dropped;
        t.duplicated += s.duplicated;
        t.messages += s.messages;
        t.leaders += s.leaders;
        t.replays += s.replays;
        t.crashes_mid += s.crashes_mid;
        t.hedges += s.hedges;
        t.missed += s.missed;
        t.suspects += s.suspects;
        t.forwarded += s.forwarded;
        t.call_failures += s.call_failures;
        t.step_downs += s.step_downs;
        t.holds += s.holds;
        t.gray += s.gray_faults;
        t.write_failures += s.write_failures;
        t.early_lost += s.early_lost;
        t.ghosts += s.ghosts;
        t.id_blocks += s.id_blocks;
        t.id_blocks_committed += s.id_blocks_committed;
        t.records += s.records;
        t.records_committed += s.records_committed;
        t.compacted += s.compacted;
        t.catchups += s.catchups;
        t.early_asks += s.early_asks;
        t.deps_entries += s.deps_entries;
        t.rereads += s.rereads;
        t.joins += s.joins;
        t.pipelined += s.pipelined;
        t.late_appends += s.late_appends;
        // BF-14: a missed receipt is hedged only by `suspect`; no command reaches a second member.
        assert_eq!(s.forwarded, 0, "seed {seed}: a command was forwarded");
    }
    if let Some(p) = out {
        std::fs::write(p, file).expect("write history");
    }
    eprintln!(
        "seeds {} steps {} ops {} acked {} refused {} reads {} no-reply {} replays {} crashes {} (mid-input {}) \
         partitions {} hedges {} messages {} dropped {} duplicated {} leaders {} | missed receipts {} suspect {} \
         forwarded {} call failures {} step-downs {} holds {} gray faults {} write failures {} | leader writes lost \
         after their AppendEntries left {} (a follower stored one the leader then lacked: {}) | id blocks recorded {} \
         committed {} | recorded entries {} committed {} | compacted members seen {} catch-up reads {} | entries \
         decided on reads {} early asks {} re-reads {} late joins {} | pipelined appends {} appends received late or twice {}",
        t.seeds,
        t.steps,
        t.ops,
        t.acked,
        t.refused,
        t.reads,
        t.gave_up,
        t.replays,
        t.crashes,
        t.crashes_mid,
        t.partitions,
        t.hedges,
        t.messages,
        t.dropped,
        t.duplicated,
        t.leaders,
        t.missed,
        t.suspects,
        t.forwarded,
        t.call_failures,
        t.step_downs,
        t.holds,
        t.gray,
        t.write_failures,
        t.early_lost,
        t.ghosts,
        t.id_blocks,
        t.id_blocks_committed,
        t.records,
        t.records_committed,
        t.compacted,
        t.catchups,
        t.deps_entries,
        t.early_asks,
        t.rereads,
        t.joins,
        t.pipelined,
        t.late_appends
    );
    eprintln!(
        "longest stretch with a healthy quorum, requests waiting, and none completing (ms, seed, from): {:?}",
        t.stuck
    );
    t
}

#[test]
fn quiet_network_commits_and_reads() {
    let t = sweep(0..4, 2_000, Faults::NONE);
    assert!(t.acked > 100, "only {} writes acked", t.acked);
    assert!(t.reads > 50, "only {} reads", t.reads);
    assert_eq!(t.gave_up, 0);
}

#[test]
fn harsh_network_keeps_every_invariant() {
    let t = sweep(1_000..1_000 + SEEDS, STEPS, Faults::HARSH);
    // The sweep must have done work under faults, or it proves nothing. HARSH acks about 20 writes a seed (1,196 to
    // 1,203 over these seeds as the pipeline moved timing, 2,293 to 2,406 over twice as many), so the floor is 15.
    assert!(t.acked > SEEDS * 15, "only {} writes acked", t.acked);
    assert!(t.crashes > SEEDS, "only {} crashes", t.crashes);
    assert!(t.partitions > SEEDS / 2, "only {} partitions", t.partitions);
    // The leader's own write lost after its AppendEntries left, with a follower storing the entry it lost.
    assert!(t.ghosts > 0, "no follower stored an entry its leader lost ({} early losses)", t.early_lost);
    assert!(t.id_blocks_committed > SEEDS, "only {} id blocks committed", t.id_blocks_committed);
    assert!(t.records_committed > SEEDS, "only {} recorded entries committed", t.records_committed);
    // Members compact their in-memory log, and followers behind the window are caught up from storage.
    assert!(t.compacted > 0 && t.catchups > SEEDS, "compacted {} catch-ups {}", t.compacted, t.catchups);
    // Dependency reads overlap the entry in flight, and a recorded entry moving the dependency forces a re-read.
    assert!(t.early_asks > SEEDS && t.rereads > 0, "early asks {} re-reads {}", t.early_asks, t.rereads);
    // PIPELINE: calls leave behind earlier ones still out, and reordering and duplicates deliver them late or twice.
    assert!(t.pipelined > SEEDS && t.late_appends > SEEDS, "pipelined {} late {}", t.pipelined, t.late_appends);
}

/// HARSH plus gray faults on one site at a time: calls failing 30%, storage confirms 30% at 1 s, or writes failing 30%.
#[test]
fn gray_network_keeps_every_invariant() {
    let t = sweep(5_000..5_000 + SEEDS / 2, STEPS, Faults::GRAY);
    assert!(t.acked > SEEDS * 10, "only {} writes acked", t.acked);
    assert!(t.gray > SEEDS / 4, "only {} gray faults", t.gray);
    assert!(t.missed > 0 && t.suspects > 0, "no receipt missed its deadline");
    assert!(t.step_downs > 5, "only {} STEP_DOWN transfers", t.step_downs);
}

/// F67: half-hung sites, transfers, drains, quick resets, and the hosted edge's follower reads and damped reports,
/// with every invariant, linearizability, and the liveness bound. The core before F67's fixes left seed 7180
/// leaderless with requests waiting for 32.6 s.
#[test]
fn live_network_elects_within_the_bound() {
    let t = sweep_with(7_160..7_160 + SEEDS / 2, STEPS, live_faults);
    assert!(t.reads > SEEDS * 20, "only {} reads", t.reads);
    assert!(t.gray > SEEDS / 4, "only {} gray faults", t.gray);
}

/// F67: a member refused as `Suspected` that stopped campaigning for 2 x SUSPECT's window outlasted the voter's bar
/// and left this seed's group leaderless with requests waiting for 31.6 s; it must keep campaigning by pre-vote.
#[test]
fn a_member_refused_as_suspected_keeps_campaigning() {
    sweep(10_395..10_396, 10_000, Faults::GRAY);
}

/// F67: a follower whose read-index call failed forgot its half-hung leader, so reports about it stopped counting and
/// its promise to it kept renewing (seed 7337: 47.8 s); and that leader's appends counted as a leader seen, so the
/// 2 x ELECTION_AFTER waiver never freed two healthy voters that quarantined each other (seed 11089: 32.9 s).
#[test]
fn a_half_hung_leader_cannot_hold_its_followers() {
    for seed in [7_337, 11_089] {
        sweep_with(seed..seed + 1, 8_000, live_faults);
    }
}

/// F67 rule 10: main left these two live seeds leaderless with requests waiting for 33.0 s and 37.4 s, past the
/// 2,000 seeds the long sweep then ran: a drained or stepping-down leader handed leadership to a member it refused in
/// pre-vote as `Suspected` (`a_leader_hands_off_to_no_member_it_suspects`).
#[test]
fn a_leader_hands_off_to_no_member_it_suspects_live() {
    for seed in [12_048, 12_073] {
        sweep_with(seed..seed + 1, 10_000, live_faults);
    }
}

/// progress-suspect: main 9ecd8314 left this live seed's healthy quorum with requests waiting for 31.8 s. A half-hung
/// leader's read-beat rounds, empty behind its entry in flight, reached both followers every 2 s while every call
/// failed, and `late-suspect` read any accepted append as the leader reaching them, so no report counted and their
/// promises held until it stepped down. A report counts once no append stored an entry or moved the commit index.
#[test]
fn a_half_hung_leader_s_empty_rounds_do_not_hold_its_followers() {
    sweep_with(10_353..10_354, 10_000, live_faults);
}

/// Seeds the long sweep's live half runs at least, from 10,000, whatever `RAFT_SEEDS` says: main passed the bound on
/// 10,000 to 11,999 while 12,048 (33.0 s) and 12,073 (37.4 s) broke it, so a 2,000-seed window hid an F67 path
/// (`tests/m0/results/f67.md`, "Rule 10 and the wider window").
const LIVE_SEEDS_MIN: u64 = 4_000;

/// The documented run: `RAFT_SEEDS=2000 RAFT_STEPS=10000 RAFT_HISTORY=path cargo test -p graftyard-raft --release
/// --test sim long_sweep -- --ignored --nocapture`, then `tests/m0/porcupine -seg 2000` on `path`, `path.gray`, and
/// `path.live`. HARSH and GRAY run `RAFT_SEEDS`; the live half runs at least LIVE_SEEDS_MIN.
#[test]
#[ignore = "long sweep: RAFT_SEEDS, RAFT_STEPS"]
fn long_sweep() {
    let seeds = env("RAFT_SEEDS", 1_000);
    let steps = env("RAFT_STEPS", 10_000);
    sweep(10_000..10_000 + seeds, steps, Faults::HARSH);
    // The same seeds count again under gray faults, then under F67's live mixes, whose histories go to `path.live`.
    sweep(10_000..10_000 + seeds, steps, Faults::GRAY);
    sweep_with(10_000..10_000 + seeds.max(LIVE_SEEDS_MIN), steps, live_faults);
}

fn row(client: usize, call: u64, ret: u64, op: HOp, out: Option<HOut>) -> Row {
    Row { client, call, ret, op, out, ack: None }
}

fn put(v: &str) -> HOp {
    HOp::Put { key: "k".into(), value: v.into() }
}

fn get() -> HOp {
    HOp::Get { key: "k".into() }
}

fn val(v: Option<&str>) -> Option<HOut> {
    Some(HOut::Value(v.map(str::to_owned)))
}

/// The checker must reject histories that are not linearizable, or a passing run means nothing.
#[test]
fn checker_rejects_known_bad_histories() {
    // A read of a value overwritten before the read began.
    let stale = [
        row(0, 0, 10, put("a"), Some(HOut::Ok(true))),
        row(0, 20, 30, put("b"), Some(HOut::Ok(true))),
        row(1, 40, 50, get(), val(Some("a"))),
    ];
    assert_ne!(history::check(&stale), Verdict::Ok);
    // A read of a value nobody wrote.
    assert_ne!(history::check(&[row(0, 0, 10, get(), val(Some("z")))]), Verdict::Ok);
    // Two sequential reads that go backwards while a concurrent write runs.
    let back = [
        row(0, 0, 100, put("b"), Some(HOut::Ok(true))),
        row(1, 10, 20, get(), val(Some("b"))),
        row(1, 30, 40, get(), val(None)),
    ];
    assert_ne!(history::check(&back), Verdict::Ok);
    // A compare-and-set that succeeded against the wrong value.
    let cas = [
        row(0, 0, 10, put("a"), Some(HOut::Ok(true))),
        row(0, 20, 30, HOp::Cas { key: "k".into(), expect: Some("x".into()), value: "y".into() }, Some(HOut::Ok(true))),
    ];
    assert_ne!(history::check(&cas), Verdict::Ok);
    // A write applied twice: an append-like double put shows as a value returning after being overwritten.
    let twice = [
        row(0, 0, 10, put("a"), Some(HOut::Ok(true))),
        row(1, 20, 30, put("b"), Some(HOut::Ok(true))),
        row(2, 40, 50, get(), val(Some("a"))),
    ];
    assert_ne!(history::check(&twice), Verdict::Ok);
}

#[test]
fn checker_accepts_concurrent_and_unknown_outcomes() {
    let ok = [
        row(0, 0, 100, put("a"), Some(HOut::Ok(true))),
        row(1, 10, 20, get(), val(Some("a"))),
        row(2, 30, 40, get(), val(Some("a"))),
        // No reply: may take effect any time after its call, or never.
        row(3, 50, 60, put("b"), None),
        row(1, 200, 210, get(), val(Some("b"))),
        row(2, 220, 230, get(), val(Some("b"))),
    ];
    assert_eq!(history::check(&ok), Verdict::Ok);
}
