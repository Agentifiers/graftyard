//! B0-32's burst replayed on a model of one site's four shards, to show where a push's time goes when it validates on
//! the disk as it stands (`validate_on`), against main's order, which caught each shard up first. The arrivals, the
//! shard each push landed on, and the container's own work per push are m3fr5c's 651 burst validates
//! (`b032_m3fr5c.csv`, from Workers Logs, 2026-11-15 04:01Z); a catch-up costs 412 ms plus 37 ms per pack it ingests,
//! the fit to that run's 182 catch-ups (`tests/am1/results/disk-validate.md`). Every acked push is a pack each shard
//! ingests 700 ms later, and main moves every 6 s, so a push whose base is the latest main finds it missing on a disk
//! that has not caught up past that move. The model is an inference from that run, not a measurement of the fix.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use graftyard_git::{CatchUpStep, CatchUps, ValidateOn, validate_on};

const RUN0: u64 = 412;
const PER_PACK: u64 = 37;
const COMMIT_MS: u64 = 700;
const FORWARD_MS: u64 = 150;
const MERGE_EVERY: u64 = 6_000;
const SHARDS: [&str; 4] = ["net", "pool4", "pool5", "pool6"];

#[derive(Default)]
struct Shard {
    runs: CatchUps,
    /// The time the running or last catch-up read the head at, and the last one that ended.
    covers: u64,
    applied: u64,
    waiters: Vec<(usize, u64, Stage)>,
}

#[derive(Clone, Copy, Debug)]
enum Stage {
    Arrive,
    AtShard,
    Work { retry: bool },
    Ack,
}

#[derive(Clone, Copy)]
enum Ev {
    RunEnd(usize),
    Push(usize, Stage),
}

struct Replay {
    on: ValidateOn,
    pushes: Vec<(u64, usize, u64)>,
    shards: Vec<Shard>,
    commits: Vec<u64>,
    queue: BinaryHeap<Reverse<(u64, u64, usize)>>,
    events: Vec<Ev>,
    acked: Vec<Option<u64>>,
    from: Vec<u64>,
    missed_again: usize,
    held: usize,
}

impl Replay {
    fn new(on: ValidateOn) -> Replay {
        let pushes: Vec<(u64, usize, u64)> = include_str!("b032_m3fr5c.csv")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                let f: Vec<&str> = l.split(',').collect();
                (f[0].parse().unwrap(), SHARDS.iter().position(|s| *s == f[1]).unwrap(), f[2].parse().unwrap())
            })
            .collect();
        let n = pushes.len();
        Replay {
            on,
            commits: (1..20).map(|k| k * MERGE_EVERY).collect(),
            shards: (0..SHARDS.len()).map(|_| Shard::default()).collect(),
            queue: BinaryHeap::new(),
            events: Vec::new(),
            acked: vec![None; n],
            from: vec![0; n],
            missed_again: 0,
            held: 0,
            pushes,
        }
    }

    fn at(&mut self, t: u64, e: Ev) {
        let seq = self.events.len();
        self.events.push(e);
        self.queue.push(Reverse((t, seq as u64, seq)));
    }

    /// `CatchUps::start` with the run's cost: every pack committed since the last run read the head.
    fn start(&mut self, s: usize, t: u64) {
        if self.shards[s].runs.start() {
            let since = self.shards[s].covers;
            let packs = self.commits.iter().filter(|c| **c > since && **c <= t).count() as u64;
            self.shards[s].covers = t;
            self.at(t + RUN0 + PER_PACK * packs, Ev::RunEnd(s));
        }
    }

    /// The site's `Jobs::start_from` then `wait_from`: on until a run numbered `from` or later ended.
    fn wait_from(&mut self, s: usize, from: u64, t: u64, i: usize, then: Stage) {
        match self.shards[s].runs.step(from) {
            CatchUpStep::Done => self.at(t, Ev::Push(i, then)),
            CatchUpStep::Start => {
                self.start(s, t);
                self.shards[s].waiters.push((i, from, then));
            }
            CatchUpStep::Await => self.shards[s].waiters.push((i, from, then)),
        }
    }

    fn base(&self, i: usize) -> u64 {
        self.pushes[i].0 / MERGE_EVERY * MERGE_EVERY
    }

    fn push(&mut self, i: usize, stage: Stage, t: u64) {
        let (_, shard, work) = self.pushes[i];
        let disk = self.on == ValidateOn::Disk;
        match stage {
            // The network's first shard: main's order catches it up before it forwards.
            Stage::Arrive => {
                let from = self.shards[0].runs.next();
                self.start(0, t);
                match (disk, shard) {
                    (true, 0) => self.at(t, Ev::Push(i, Stage::AtShard)),
                    (true, _) => self.at(t + FORWARD_MS, Ev::Push(i, Stage::AtShard)),
                    (false, 0) => self.wait_from(0, from, t, i, Stage::Work { retry: true }),
                    (false, _) => self.wait_from(0, from, t, i, Stage::AtShard),
                }
            }
            Stage::AtShard => {
                let from = self.shards[shard].runs.next();
                self.from[i] = from;
                self.start(shard, t);
                let t = if !disk && shard != 0 { t + FORWARD_MS } else { t };
                if disk {
                    self.at(t, Ev::Push(i, Stage::Work { retry: false }));
                } else {
                    self.wait_from(shard, from, t, i, Stage::Work { retry: true });
                }
            }
            Stage::Work { retry } => {
                let missing = self.shards[shard].applied < self.base(i);
                if missing && !retry {
                    // The container holds the request; a run started after it arrived ingests the base.
                    self.held += 1;
                    let from = self.from[i];
                    self.wait_from(shard, from, t + work, i, Stage::Work { retry: true });
                } else {
                    if missing {
                        self.missed_again += 1;
                    }
                    self.at(t + work, Ev::Push(i, Stage::Ack));
                }
            }
            Stage::Ack => {
                self.acked[i] = Some(t);
                self.commits.push(t + COMMIT_MS);
            }
        }
    }

    fn run(mut self) -> Replay {
        for i in 0..self.pushes.len() {
            self.at(self.pushes[i].0, Ev::Push(i, Stage::Arrive));
        }
        while let Some(Reverse((t, _, e))) = self.queue.pop() {
            match self.events[e] {
                Ev::RunEnd(s) => {
                    self.shards[s].applied = self.shards[s].covers;
                    self.shards[s].runs.end(true);
                    for (i, from, then) in std::mem::take(&mut self.shards[s].waiters) {
                        self.wait_from(s, from, t, i, then);
                    }
                }
                Ev::Push(i, stage) => self.push(i, stage, t),
            }
        }
        self
    }

    /// Each push's time from its arrival at the network's first shard to its validate's answer, sorted.
    fn walls(&self) -> Vec<u64> {
        let mut w: Vec<u64> =
            self.acked.iter().zip(&self.pushes).map(|(a, p)| a.expect("every push answered") - p.0).collect();
        w.sort_unstable();
        w
    }
}

fn pct(w: &[u64], p: f64) -> u64 {
    w[((w.len() - 1) as f64 * p) as usize]
}

/// On m3fr5c's burst, validating on the disk as it stands brings the shard's validate p50 to about 1 s, against
/// about 4.6 s in main's order, and a push held on a missing base is checked again only after a catch-up that read
/// the head after it arrived, so none misses twice.
#[test]
fn b032s_burst_validates_in_about_a_second_on_the_disk_as_it_stands() {
    let on = validate_on(false, true, true);
    let fix = Replay::new(on).run();
    let main = Replay::new(ValidateOn::CaughtUp).run();
    let (w, m) = (fix.walls(), main.walls());
    eprintln!(
        "disk: p50 {} p90 {} p99 {} ms, {} of {} within 5 s, {} held; main's order: p50 {} p90 {} ms, {} within 5 s",
        pct(&w, 0.5),
        pct(&w, 0.9),
        pct(&w, 0.99),
        w.iter().filter(|x| **x <= 5_000).count(),
        w.len(),
        fix.held,
        pct(&m, 0.5),
        pct(&m, 0.9),
        m.iter().filter(|x| **x <= 5_000).count()
    );
    assert_eq!(w.len(), 651);
    assert!(pct(&w, 0.5) <= 1_200, "disk p50 {} ms", pct(&w, 0.5));
    assert!(pct(&m, 0.5) >= 4_000, "main's order p50 {} ms", pct(&m, 0.5));
    assert!(fix.held > 0, "the replay never found a base missing");
    assert_eq!(fix.missed_again, 0, "a held push was checked again before a catch-up that read its base");
    assert_eq!(main.missed_again, 0);
}

/// A push validates on the disk as it stands only from a running container this isolate caught up; the archive's
/// promotion always catches up first.
#[test]
fn only_a_caught_up_running_container_validates_on_its_disk() {
    assert_eq!(validate_on(false, true, true), ValidateOn::Disk);
    assert_eq!(validate_on(true, true, true), ValidateOn::CaughtUp);
    assert_eq!(validate_on(false, false, true), ValidateOn::CaughtUp);
    assert_eq!(validate_on(false, true, false), ValidateOn::CaughtUp);
}

/// A caller's catch-up is the first to start after it: one running when it arrives may have read the head before
/// the caller's base committed, so it waits for that one to end and then for the next; one that ended after a
/// later start already covers it; a failed run covers no one.
#[test]
fn a_caller_waits_for_the_first_catch_up_that_starts_after_it() {
    let mut c = CatchUps::default();
    let from = c.next();
    assert_eq!(c.step(from), CatchUpStep::Start);
    assert!(c.start());
    assert_eq!(c.step(from), CatchUpStep::Await);
    // A second caller arrives while run 1 runs: it needs run 2.
    let later = c.next();
    assert_eq!(later, 2);
    assert!(!c.start(), "one at a time");
    c.end(true);
    assert_eq!(c.step(from), CatchUpStep::Done);
    assert_eq!(c.step(later), CatchUpStep::Start);
    assert!(c.start());
    c.end(false);
    assert_eq!(c.step(later), CatchUpStep::Start, "a failed run covers no one");
    assert!(c.start());
    c.end(true);
    assert_eq!(c.step(later), CatchUpStep::Done);
}
