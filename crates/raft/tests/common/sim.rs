//! A deterministic simulator of one three-member group.
//!
//! Fault model, all drawn from one seeded generator: each message may be dropped, delayed by 5 ms to 1.5 s (so
//! messages reorder), or duplicated; a member may crash, losing every write whose storage confirm has not ended and
//! every output still held behind its output gate, and restart from its `Stored` after a delay; and the sites may
//! split into two sides that pass no messages for a while. A write reaches the member's disk only when its confirm
//! ends, 2 to 22 ms after the input (or a gray site's slow confirm). The leader's AppendEntries (`Out::early`) leave
//! at once, and half the mid-input crashes land after they left, so a follower may hold an entry its leader lost.
//! The clock advances in steps of 1 to 50 ms. Clients are edges: they mint one request id per operation and re-send it after `NotLeader`,
//! a timeout, or a lost member, and hedge only with `campaign` (the `suspect` report) to a member. Half the puts
//! depend on another chain (`KvCmd::PutDep`), whose read the host answers 0 to 400 ms after the member asks, sometimes
//! twice; a put is never refused for the wrong dependency set, and every command an entry decided on reads was
//! admitted no later than the first of those reads was asked for.
//!
//! After every step `check` asserts the invariants.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use graftyard_raft::{
    Admitted, Alarm, ByRequest, CallError, CallFailure, Colo, CommitError, ConfirmId, Digest, EntryBody, Envelope,
    Index, Member, MemberConfig, Message, Out, ReadId, Receipt, ReleaseTag, RequestId, Resolution, Role, SelfAck,
    Stored, Term, Ticket, Write,
};
use graftyard_types::{ChainId, DbId, PlacementName, PlacementRecord, Region, ReleaseId, RepoId, SiteId, Timestamp};

use super::history::{Ack, HOp, HOut, Row};
use super::kv::{KV_FORMAT, Kv, KvCmd, KvErr, KvEvent};

pub const SITES: [SiteId; 3] = [SiteId(0), SiteId(1), SiteId(2)];
pub const CHAIN: ChainId = ChainId::Repo(RepoId(DbId(42)));

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) ^ 0x1234_5678)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    /// True with probability p per mille.
    pub fn chance(&mut self, permille: u64) -> bool {
        self.below(1000) < permille
    }
}

pub fn record() -> PlacementRecord {
    PlacementRecord {
        name: PlacementName::Tri,
        sites: SITES.iter().copied().collect(),
        home: SITES[0],
        generation: 1,
        moving_from: None,
        at: Timestamp(1),
    }
}

pub fn config(me: SiteId) -> MemberConfig {
    MemberConfig {
        chain: CHAIN,
        me,
        record: record(),
        regions: vec![(SITES[0], Region::Enam), (SITES[1], Region::Weur), (SITES[2], Region::Apac)],
        release: ReleaseTag { id: ReleaseId([7; 32]), seq: 1 },
        reads: vec![KV_FORMAT],
    }
}

/// Fault rates in per mille, and how often each fault starts.
#[derive(Clone, Copy, Debug)]
pub struct Faults {
    pub drop: u64,
    pub dup: u64,
    /// Per step, per mille, that a live member crashes.
    pub crash: u64,
    /// Per step, per mille, that a partition starts.
    pub partition: u64,
    pub max_delay_ms: u64,
    /// Per input, per mille, that the member crashes before the input's writes persist: the writes and every output
    /// of that input are lost, except, half the time, its `early` AppendEntries, which already left.
    pub crash_mid: u64,
    /// Per step, per mille, that an edge sends a stray `suspect` report to a random member.
    pub hedge: u64,
    /// Per step, per mille, that a gray fault (`FaultKind`, drawn at random) starts on a random site, for 5 to 30 s.
    pub gray: u64,
    /// Per step, per mille, that a member with a storage confirm outstanding crashes: its unconfirmed writes are
    /// lost, while a leader's AppendEntries for them already left.
    pub crash_unconfirmed: u64,
    /// Per step, per mille, that the host asks the leader to transfer to a random other voter (`/transfer`, as
    /// B0-30's `keepHome` did): a `STEP_DOWN` with `to`.
    pub transfer: u64,
    /// Per step, per mille, that a random site is drained for 2 to 20 s: the leader there hands off with no `to`
    /// named, and its members may not lead (`SiteMode`).
    pub drain: u64,
    /// When a member crashes, the most it stays down, in ms (4,000 unless set): an isolate eviction is back at once.
    pub down_max_ms: u64,
    /// Gray faults may also be `HalfHung` (300 per mille).
    pub half_hung: bool,
}

impl Faults {
    pub const NONE: Faults = Faults {
        drop: 0,
        dup: 0,
        crash: 0,
        partition: 0,
        max_delay_ms: 20,
        crash_mid: 0,
        hedge: 0,
        gray: 0,
        crash_unconfirmed: 0,
        transfer: 0,
        drain: 0,
        down_max_ms: 0,
        half_hung: false,
    };
    pub const HARSH: Faults = Faults {
        drop: 80,
        dup: 40,
        crash: 3,
        partition: 3,
        max_delay_ms: 1500,
        crash_mid: 2,
        hedge: 20,
        gray: 0,
        crash_unconfirmed: 20,
        transfer: 0,
        drain: 0,
        down_max_ms: 0,
        half_hung: false,
    };
    /// Gray faults (a site whose calls fail, whose storage confirms are slow, or whose writes fail, one at a time)
    /// with partitions, duplicates, reordering, 1% loss, and no crashes but those the failing writes cause. HARSH's
    /// crash rate puts nearly every member inside RESET_QUARANTINE by its two boots, so every trip would hold and no
    /// `STEP_DOWN` transfer would run; HARSH covers crashes.
    pub const GRAY: Faults = Faults {
        drop: 10,
        dup: 40,
        crash: 0,
        partition: 1,
        max_delay_ms: 400,
        crash_mid: 0,
        hedge: 20,
        gray: 2,
        crash_unconfirmed: 0,
        transfer: 0,
        drain: 0,
        down_max_ms: 0,
        half_hung: false,
    };
    /// No faults, one-way delays of 5 to 150 ms: an intercontinental group at rest, for the liveness runs.
    pub const WAN: Faults = Faults { max_delay_ms: 150, ..Faults::NONE };
}

/// A gray fault on one site from `from` until `until` (`tests/m0/results/m0-wp4.md`, B0-06's three modes).
#[derive(Clone, Copy, Debug)]
pub struct SiteFault {
    pub site: SiteId,
    pub from: Timestamp,
    pub until: Timestamp,
    pub kind: FaultKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// Each member call to or from the site, and each edge receipt from it, fails with this chance per mille.
    Calls(u64),
    /// Each storage confirm on the site takes `ms` with this chance per mille, and holds the site's outputs meanwhile
    /// (the output gate).
    SlowConfirm(u64, u64),
    /// Each input with storage writes on the site fails with this chance per mille, which resets the member.
    FailWrites(u64),
    /// Half-hung: each call to or from the site runs, but its answer is lost with this chance per mille, so the caller
    /// sees a missed deadline (B0-07's hung calls, `drop_append_reply`). An edge's `suspect` to the site still starts
    /// a campaign there; the edge never hears the verdict.
    HalfHung(u64),
}

/// The hosted edge's lookup (`crates/lookup`), which the default simulator does not model (F67).
#[derive(Clone, Copy, Debug, Default)]
pub struct Edge {
    /// A get goes first to a follower by read-index (`read_spread`). A follower that has not answered within
    /// FOLLOWER_WAIT (1 s) answers `overloaded`, and the read fails as a 503 with no `suspect` report; only a
    /// follower's `NotLeader` sends it on to the leader's path, which reports.
    pub follower_reads: bool,
    /// One client sends the same report (about, term, scope) at most once per this many ms (`SUSPECT_GAP`).
    pub suspect_gap_ms: u64,
    /// Clients only read: an `Authz` group under API load, whose writes are rare (LM-1 round 5).
    pub reads_only: bool,
}

/// FOLLOWER_WAIT in `workers/site/src/member.rs`.
pub const FOLLOWER_WAIT_MS: u64 = 1_000;
/// Granted spread reads answered later through `Member::fresh_after`, per mille: a learner's narrow API answer whose
/// build was not due (`workers/site/src/member.rs`).
const LATER_SHARE: u64 = 300;

/// When a granted spread read `id` is answered later (LATER_SHARE of them), the ms after its grant, under
/// FOLLOWER_WAIT_MS.
fn later_ms(id: ReadId) -> Option<u64> {
    let h = id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    (h % 1000 < LATER_SHARE).then_some((h >> 12) % FOLLOWER_WAIT_MS)
}

/// A member-to-member call the host is timing: its failure report, and when its deadline passes.
struct PendingCall {
    caller: SiteId,
    failure: CallFailure,
    due: Timestamp,
}

/// What `RAFT_TRACE` prints when it changes: role, term, `promise_until`, and `transfer`.
type Traced = (Role, Term, u64, Option<graftyard_raft::TransferId>);

/// The reply that answers a pending call: (caller, callee, kind, a, b, c).
type CallKey = (SiteId, SiteId, u8, u64, u64);

fn request_key<E>(e: &Envelope<E>) -> Option<CallKey> {
    match &e.msg {
        Message::Append(a) => Some((e.from, e.to, 0, a.boot, a.round)),
        Message::Ping { probe, n } => Some((e.from, e.to, 1, *probe, u64::from(*n))),
        _ => None,
    }
}

fn reply_key<E>(e: &Envelope<E>) -> Option<CallKey> {
    match &e.msg {
        Message::AppendRes(r) => Some((e.to, e.from, 0, r.boot, r.round)),
        Message::Pong { probe, n } => Some((e.to, e.from, 1, *probe, u64::from(*n))),
        _ => None,
    }
}

struct Flight {
    at: Timestamp,
    /// When it left the sender: a crash before then means it never left.
    left: Timestamp,
    env: Envelope<KvEvent>,
}

/// Collects one input's writes until its storage confirm ends.
struct Pending(Vec<Write<KvEvent>>);

impl graftyard_raft::Storage<KvEvent> for Pending {
    fn write(&mut self, w: &Write<KvEvent>) {
        self.0.push(w.clone());
    }
}

/// One storage confirm in progress: when it ends, the site and its generation, the confirm id, the writes it makes
/// durable, and the leader's `SelfAck` for them.
type ConfirmEnd = (Timestamp, SiteId, u64, ConfirmId, Vec<Write<KvEvent>>, Option<SelfAck>);

/// A member's disk: the reference `Stored`, plus each entry's digest so the invariant check does not rehash.
pub struct Disk {
    pub stored: Stored<Kv>,
    pub digests: Vec<Digest>,
}

impl graftyard_raft::Storage<KvEvent> for Disk {
    fn write(&mut self, w: &graftyard_raft::Write<KvEvent>) {
        self.stored.write(w);
        match w {
            graftyard_raft::Write::Append(e) => self.digests.push(graftyard_raft::digest(e)),
            graftyard_raft::Write::Truncate(i) => self.digests.truncate(i.0 as usize - 1),
            _ => {}
        }
    }
}

pub enum Slot {
    Up(Box<Member<KvCmd>>),
    Down { until: Timestamp },
}

/// One client operation in progress.
struct Op {
    client: usize,
    op: HOp,
    id: RequestId,
    call: Timestamp,
    target: SiteId,
    /// The ticket the target admitted, or the read it started.
    wait: Option<Wait>,
    /// When to give up on the current wait and re-send or hedge.
    retry_at: Timestamp,
    deadline: Timestamp,
    /// The last receipt from `target` missed CALL_DEADLINE: at `retry_at` the edge reports `suspect` first.
    missed: bool,
    /// The highest term the edge has seen for the group.
    term: Term,
    /// A write's command, which only `Receipt::call` releases, to the receipt's one target.
    receipt: Option<Receipt<KvCmd>>,
    /// A get already asked a follower by read-index (`Edge::follower_reads`).
    spread: bool,
}

#[derive(Clone)]
enum Wait {
    Ticket(Ticket),
    Read(ReadId),
    /// A read-index read on a follower, which sheds it at `retry_at`.
    Spread(SiteId, ReadId),
    /// A granted read-index read whose answer the follower builds later, as a narrow API answer whose build was not
    /// due: answered through `Member::fresh_after` at `retry_at`, at the follower's state then.
    Later(SiteId, graftyard_raft::ReadGrant),
}

#[derive(Default, Debug, Clone, Copy)]
pub struct Stats {
    pub steps: u64,
    pub messages: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub crashes: u64,
    pub partitions: u64,
    pub acked: u64,
    pub refused: u64,
    pub reads: u64,
    pub gave_up: u64,
    pub leaders: u64,
    pub max_commit: u64,
    pub replays: u64,
    pub crashes_mid: u64,
    pub hedges: u64,
    /// Receipts that missed CALL_DEADLINE, and the `suspect` reports edges sent.
    pub missed: u64,
    pub suspects: u64,
    /// BF-14: commands an edge sent to a member while a receipt for the same request was outstanding at another.
    pub forwarded: u64,
    pub call_failures: u64,
    pub transfers: u64,
    pub step_downs: u64,
    pub holds: u64,
    pub gray_faults: u64,
    pub write_failures: u64,
    /// Mid-input crashes after the input's AppendEntries left: the leader lost an entry a follower may hold.
    pub early_lost: u64,
    /// Restarts after which another member's disk holds an entry, of a term the restarted member led, at an index
    /// past the restarted member's own log, or different from its own there.
    pub ghosts: u64,
    /// `Member::id_block` calls the host made, and the blocks that committed.
    pub id_blocks: u64,
    pub id_blocks_committed: u64,
    /// `Member::record` calls the host made (an entry of events with no requests), and those that committed.
    pub records: u64,
    pub records_committed: u64,
    /// Checks that found a member with entries compacted out of memory, and `Out::catchup` reads served.
    pub compacted: u64,
    pub catchups: u64,
    /// `DepsWanted` asked while the asking leader had an entry of its term in flight (an early cut), entries decided
    /// on reads, and those whose batch read again after the entry in flight applied (a re-read).
    pub early_asks: u64,
    pub deps_entries: u64,
    pub rereads: u64,
    /// Gets a follower answered by read-index, and those it shed (503, no report).
    pub spread_reads: u64,
    pub shed: u64,
    /// `suspect` reports the edge's damping held back.
    pub damped: u64,
    pub drains: u64,
}

pub struct Sim {
    pub rng: Rng,
    pub now: Timestamp,
    pub slots: Vec<Slot>,
    pub disks: Vec<Disk>,
    /// Per member, the committed prefix already checked, reset when the member restarts.
    checked: [u64; 3],
    flights: Vec<Flight>,
    pub faults: Faults,
    /// Sites on side A while a partition lasts, and its end.
    partition: Option<(BTreeSet<SiteId>, Timestamp)>,
    ops: Vec<Option<Op>>,
    next_id: u128,
    next_value: u64,
    pub history: Vec<Row>,
    pub stats: Stats,
    seed: u64,
    restarts: u64,
    // Invariant state.
    leaders: BTreeMap<Term, SiteId>,
    /// Digest of every index any member has committed.
    committed: BTreeMap<u64, (Digest, Term)>,
    /// The one index at which each request id was applied, over every member.
    applied_at: BTreeMap<RequestId, u64>,
    /// Every Acked write: its entry index and digest.
    acked: Vec<(RequestId, u64, Digest, Term)>,
    /// The clients' keys.
    keys: usize,
    /// Per-member clock skew in ms.
    skew: [i64; 3],
    /// Gray faults: scheduled or drawn.
    pub site_faults: Vec<SiteFault>,
    pending_calls: BTreeMap<CallKey, PendingCall>,
    /// Per site: when its last storage confirm ends. Its outputs leave no earlier (the output gate).
    gate: [Timestamp; 3],
    confirm_ends: Vec<ConfirmEnd>,
    /// Per site, counts restarts, so a confirm of a crashed instance is not reported to its successor.
    generation: [u64; 3],
    /// Every alarm with its site and time.
    pub alarms: Vec<(Timestamp, SiteId, Alarm)>,
    /// The next whole block the host leases, and every committed id block by index.
    next_block: u32,
    id_blocks: BTreeMap<u64, graftyard_raft::IdBlock>,
    /// Per leader term: (term, site, first time seen).
    pub terms: Vec<(Term, SiteId, Timestamp)>,
    /// Every acked write: when, and the site that acked it.
    pub acks: Vec<(Timestamp, SiteId)>,
    /// Per edge request, the member whose receipt is outstanding and when its deadline passes.
    outstanding: BTreeMap<RequestId, (SiteId, Timestamp)>,
    traced: [Option<Traced>; 3],
    /// When set, every storage confirm: when, where, how long, and whether the member led.
    pub record_confirms: bool,
    pub confirm_log: Vec<(Timestamp, SiteId, u64, bool)>,
    /// `Out::catchup` requests not yet served, with the generation of the member that asked.
    catchups: Vec<(SiteId, u64, graftyard_raft::Catchup)>,
    /// Dependency reads the host owes: when it answers, the site and generation that asked, and the ask.
    dep_reads: Vec<(Timestamp, SiteId, u64, graftyard_raft::DepsWanted)>,
    /// When each `DepsWanted` was asked for, by (site, id).
    deps_asked: BTreeMap<(SiteId, u64), Timestamp>,
    /// When each request was first admitted, by (site, ticket term, ticket boot, request id).
    admitted_at: BTreeMap<(SiteId, Term, u64, RequestId), Timestamp>,
    /// Per site, the host's own record of its storage confirms: (the disk's last index once a confirm that appended
    /// ended, the site's clock then), for `Ack::self_synced_at`. Reset on restart.
    synced: [Vec<(u64, u64)>; 3],
    pub edge: Edge,
    /// Per client, the last time it sent each report (about, term, all), for `Edge::suspect_gap_ms`.
    reported: BTreeMap<(usize, SiteId, Term, bool), Timestamp>,
    /// The drained site and when its drain ends.
    drained: Option<(SiteId, Timestamp)>,
    /// The liveness monitor (F67): since when the group has had no leader while a quorum of voters was up, linked,
    /// and fault-free, and a client request was waiting; and every such stretch that ended, (from, to).
    stuck_since: Option<Timestamp>,
    stuck_set: Vec<SiteId>,
    pub leaderless: Vec<(Timestamp, Timestamp)>,
}

/// Applied entries each simulated member keeps in memory: small, so compaction and `Out::catchup` run all the time.
pub const SIM_WINDOW: u64 = 6;

/// Members' clocks differ by up to twice this, which stays under DRIFT.
pub const SKEW_MS: u64 = 200;

impl Sim {
    pub fn new(seed: u64, faults: Faults, clients: usize, keys: usize) -> Sim {
        let mut sim = Sim {
            rng: Rng::new(seed),
            now: Timestamp(1_000),
            slots: Vec::new(),
            disks: SITES.iter().map(|_| Disk { stored: Stored::new(Kv::default()), digests: Vec::new() }).collect(),
            checked: [0; 3],
            flights: Vec::new(),
            faults,
            partition: None,
            ops: (0..clients).map(|_| None).collect(),
            next_id: (seed as u128) << 64,
            next_value: 0,
            history: Vec::new(),
            stats: Stats::default(),
            seed,
            restarts: 0,
            leaders: BTreeMap::new(),
            committed: BTreeMap::new(),
            acked: Vec::new(),
            applied_at: BTreeMap::new(),
            keys,
            skew: [0; 3],
            site_faults: Vec::new(),
            pending_calls: BTreeMap::new(),
            gate: [Timestamp(0); 3],
            confirm_ends: Vec::new(),
            generation: [0; 3],
            alarms: Vec::new(),
            next_block: 1,
            id_blocks: BTreeMap::new(),
            terms: Vec::new(),
            acks: Vec::new(),
            outstanding: BTreeMap::new(),
            traced: [None; 3],
            record_confirms: false,
            confirm_log: Vec::new(),
            catchups: Vec::new(),
            dep_reads: Vec::new(),
            deps_asked: BTreeMap::new(),
            admitted_at: BTreeMap::new(),
            synced: [vec![(0, 0)], vec![(0, 0)], vec![(0, 0)]],
            edge: Edge::default(),
            reported: BTreeMap::new(),
            drained: None,
            stuck_since: None,
            stuck_set: Vec::new(),
            leaderless: Vec::new(),
        };
        for k in 0..3 {
            sim.skew[k] = sim.rng.below(2 * SKEW_MS + 1) as i64 - SKEW_MS as i64;
        }
        for s in SITES {
            let mut m = Member::new(config(s), seed ^ (u64::from(s.0) << 32)).expect("record builds");
            m.set_log_window(SIM_WINDOW);
            sim.slots.push(Slot::Up(Box::new(m)));
        }
        sim
    }

    pub fn member(&self, s: SiteId) -> Option<&Member<KvCmd>> {
        match &self.slots[s.0 as usize] {
            Slot::Up(m) => Some(m),
            Slot::Down { .. } => None,
        }
    }

    pub fn member_mut(&mut self, s: SiteId) -> Option<&mut Member<KvCmd>> {
        match &mut self.slots[s.0 as usize] {
            Slot::Up(m) => Some(m),
            Slot::Down { .. } => None,
        }
    }

    pub fn leader(&self) -> Option<SiteId> {
        SITES
            .iter()
            .copied()
            .filter(|s| self.member(*s).is_some_and(|m| m.role() == Role::Leader))
            .max_by_key(|s| self.member(*s).map(|m| m.term()))
    }

    /// The gray fault active on `s` now, if any.
    pub fn fault_on(&self, s: SiteId) -> Option<FaultKind> {
        self.site_faults.iter().find(|f| f.site == s && f.from <= self.now && self.now < f.until).map(|f| f.kind)
    }

    fn call_fails(&mut self, a: SiteId, b: SiteId) -> bool {
        let rate = |s: SiteId| match self.fault_on(s) {
            Some(FaultKind::Calls(p)) => p,
            _ => 0,
        };
        let p = rate(a).max(rate(b));
        p > 0 && self.rng.chance(p)
    }

    /// A call between `a` and `b` ran, but its answer is lost: either is half-hung.
    pub fn answer_lost(&mut self, a: SiteId, b: SiteId) -> bool {
        let rate = |s: SiteId| match self.fault_on(s) {
            Some(FaultKind::HalfHung(p)) => p,
            _ => 0,
        };
        let p = rate(a).max(rate(b));
        p > 0 && self.rng.chance(p)
    }

    /// Queues what a member released, no earlier than its output gate opens. Requests the host times (appends and
    /// pings) get a pending call whose deadline is CALL_DEADLINE after they leave.
    pub fn deliver_out(&mut self, site: SiteId, out: Out<KvCmd>) -> Vec<Resolution<KvCmd>> {
        let early = out.early.into_iter().map(|e| (true, e));
        for (early, env) in early.chain(out.messages.into_iter().map(|e| (false, e))) {
            self.stats.messages += 1;
            let from = env.from;
            let leave = if early { self.now } else { self.now.max(self.gate[from.0 as usize]) };
            if let Some(key) = request_key(&env)
                && let Some(failure) = env.failure(true)
            {
                let due = Timestamp(leave.0 + graftyard_raft::CALL_DEADLINE.as_millis() as u64);
                self.pending_calls.insert(key, PendingCall { caller: from, failure, due });
            }
            // A gray site's calls fail fast with an error, so a failed request is reported as such, not as a miss.
            // One roll per call, on its request, as Milestone 0's faulty stub failed a call as a whole.
            let is_reply = reply_key(&env).is_some()
                || matches!(env.msg, Message::VoteRes(_) | Message::Proposed { .. } | Message::ProbeRes { .. });
            let failing = !is_reply && self.call_fails(env.from, env.to);
            // A half-hung site's calls run, and their answers are lost: the caller's deadline passes.
            let answer = is_reply || matches!(env.msg, Message::TransferRefused { .. } | Message::ReadIndexRes { .. });
            if answer && self.answer_lost(env.from, env.to) {
                self.stats.dropped += 1;
                continue;
            }
            if std::env::var("RAFT_TRACE_CALLS").is_ok()
                && matches!(env.msg, Message::Append(_) | Message::AppendRes(_))
            {
                let k = if matches!(env.msg, Message::Append(_)) { "req" } else { "res" };
                eprintln!("{} msg {k} {:?} -> {:?} fail {failing}", self.now.0, env.from, env.to);
            }
            if failing {
                self.stats.dropped += 1;
                if let Some(key) = request_key(&env)
                    && let Some(mut pc) = self.pending_calls.remove(&key)
                {
                    pc.failure.deadline = false;
                    pc.due = Timestamp(leave.0 + 5 + self.rng.below(100));
                    self.pending_calls.insert(key, pc);
                }
                continue;
            }
            if self.rng.chance(self.faults.drop) {
                self.stats.dropped += 1;
                continue;
            }
            let copies = if self.rng.chance(self.faults.dup) { 2 } else { 1 };
            if copies == 2 {
                self.stats.duplicated += 1;
            }
            for _ in 0..copies {
                let delay = 5 + self.rng.below(self.faults.max_delay_ms.max(6) - 4);
                self.flights.push(Flight { at: Timestamp(leave.0 + delay), left: leave, env: env.clone() });
            }
        }
        for a in out.alarms {
            match &a {
                Alarm::StepDown { to: Some(_), .. } => self.stats.step_downs += 1,
                Alarm::StepDown { to: None, .. } => self.stats.holds += 1,
                _ => {}
            }
            self.alarms.push((self.now, site, a));
        }
        out.resolutions
    }

    /// The member's own clock: the global clock plus a fixed skew of up to +-SKEW_MS, under DRIFT.
    pub fn clock(&self, s: SiteId) -> Timestamp {
        Timestamp((self.now.0 as i64 + self.skew[s.0 as usize]) as u64)
    }

    /// Runs `f` on member `s`, persists its `Ready`, sends its outputs, and returns the reply and resolutions.
    /// A `Ready` with writes is one storage confirm, which the member is told about: its writes reach the disk, and
    /// its `SelfAck` goes back to the member, when the confirm ends; a slow one on a gray site holds the site's
    /// outputs until it ends. The `early` AppendEntries leave at once.
    pub fn call<R>(
        &mut self,
        s: SiteId,
        f: impl FnOnce(&mut Member<KvCmd>, Timestamp) -> graftyard_raft::Ready<KvCmd, R>,
    ) -> Option<(R, Vec<Resolution<KvCmd>>)> {
        let now = self.clock(s);
        let i = s.0 as usize;
        let Slot::Up(m) = &mut self.slots[i] else { return None };
        let ready = f(m, now);
        let has_writes = !ready.writes.is_empty();
        let fail_write = matches!(self.fault_on(s), Some(FaultKind::FailWrites(p)) if has_writes && self.rng.chance(p));
        if fail_write || self.rng.chance(self.faults.crash_mid) {
            // The object reset before its writes were confirmed: nothing of this input survives. Half the time its
            // AppendEntries had already left.
            let (_, out) = ready.persist(&mut Pending(Vec::new()));
            if fail_write {
                self.stats.write_failures += 1;
            } else {
                self.stats.crashes_mid += 1;
            }
            if !out.early.is_empty() && self.rng.chance(500) {
                self.stats.early_lost += 1;
                let early = Out { early: out.early, ..Out::default() };
                self.deliver_out(s, early);
            }
            self.crash(s);
            return None;
        }
        let mut pending = Pending(Vec::new());
        let (reply, mut out) = ready.persist(&mut pending);
        if has_writes {
            let mut ms = 2 + self.rng.below(20);
            if let Some(FaultKind::SlowConfirm(p, slow)) = self.fault_on(s)
                && self.rng.chance(p)
            {
                ms = slow;
            }
            let Slot::Up(m) = &mut self.slots[i] else { unreachable!() };
            let id = m.confirm_begin(now);
            if self.record_confirms {
                self.confirm_log.push((self.now, s, ms, m.role() == Role::Leader));
            }
            // Confirms overlap and complete in order: this one ends after its own time and after every earlier one.
            let end = Timestamp((self.now.0 + ms).max(self.gate[i].0));
            self.gate[i] = end;
            self.confirm_ends.push((end, s, self.generation[i], id, pending.0, out.self_ack.take()));
        }
        let g = self.generation[i];
        self.catchups.extend(std::mem::take(&mut out.catchup).into_iter().map(|c| (s, g, c)));
        self.deps_out(s, &mut out);
        let res = self.deliver_out(s, out);
        Some((reply, res))
    }

    /// The host's side of dependency reads: each ask is answered later, after the input's output gate, and each entry
    /// decided on reads is checked to hold only commands admitted no later than the first of those reads was asked.
    fn deps_out(&mut self, s: SiteId, out: &mut Out<KvCmd>) {
        let i = s.0 as usize;
        for want in std::mem::take(&mut out.deps) {
            self.deps_asked.insert((s, want.id), self.now);
            let m = self.member(s).expect("up");
            let flight = m.last_index() > m.state().commit
                && m.entry_at(m.last_index()).is_some_and(|e| e.term == m.term() && !matches!(e.body, EntryBody::Noop));
            self.stats.early_asks += u64::from(flight);
            let copies = if self.rng.chance(50) { 2 } else { 1 };
            for _ in 0..copies {
                let at = Timestamp(self.now.max(self.gate[i]).0 + self.rng.below(400));
                self.dep_reads.push((at, s, self.generation[i], want.clone()));
            }
        }
        let seed = self.seed;
        for used in std::mem::take(&mut out.deps_used) {
            self.stats.deps_entries += 1;
            self.stats.rereads += u64::from(used.ids.len() > 1);
            let asked = used
                .ids
                .iter()
                .map(|id| *self.deps_asked.get(&(s, *id)).unwrap_or_else(|| panic!("seed {seed}: {id} never asked")))
                .min()
                .expect("an entry decided on reads names one");
            let m = self.member(s).expect("up");
            let e = m.entry_at(used.index).expect("the decided entry is held");
            for (rid, _) in &e.requests {
                let at = self.admitted_at.get(&(s, e.term, m.boot(), *rid)).copied();
                assert!(
                    at.is_some_and(|t| t <= asked),
                    "seed {seed}: {s:?} entry {:?} decided {rid:?}, admitted at {at:?}, on reads asked at {asked:?}",
                    used.index
                );
            }
        }
    }

    /// Answers the dependency reads due now with each chain's fold, which no test revokes.
    fn answer_deps(&mut self) {
        let now = self.now;
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.dep_reads).into_iter().partition(|d| d.0 <= now);
        self.dep_reads = keep;
        for (_, s, g, want) in due {
            if g != self.generation[s.0 as usize] {
                continue;
            }
            let deps: Vec<_> = want.chains.iter().map(|c| (*c, Kv::default())).collect();
            if let Some((_, res)) = self.call(s, |m, t| m.deps_read(t, want.id, deps)) {
                self.handle(s, res);
            }
        }
    }

    /// A follower behind the window: the host reads the entries from its disk. Entries whose confirm is still
    /// outstanding are not there yet; the member asks again on a later round.
    fn serve_catchup(&mut self, s: SiteId, c: graftyard_raft::Catchup) -> Vec<Resolution<KvCmd>> {
        self.stats.catchups += 1;
        let log = &self.disks[s.0 as usize].stored.log;
        let from = c.from.0.max(1) as usize;
        if (c.through.0 as usize) > log.len() {
            return Vec::new();
        }
        let entries = log[from - 1..c.through.0 as usize].to_vec();
        self.call(s, move |m, t| m.catchup(t, c, entries)).map(|r| r.1).unwrap_or_default()
    }

    /// The digest and body of the entry at `i` on member `s`: from its memory, or below its window from its disk.
    fn held(&self, s: SiteId, i: u64) -> Option<(Digest, EntryBody<KvEvent>)> {
        let m = self.member(s)?;
        match m.entry_at(Index(i)) {
            Some(e) => Some((m.digest_of(Index(i))?, e.body.clone())),
            None if i <= m.log_base().index.0 => {
                let d = &self.disks[s.0 as usize];
                Some((*d.digests.get(i as usize - 1)?, d.stored.log.get(i as usize - 1)?.body.clone()))
            }
            None => None,
        }
    }

    fn crash(&mut self, s: SiteId) {
        let max = if self.faults.down_max_ms > 0 { self.faults.down_max_ms } else { 4_000 };
        let down = Timestamp(self.now.0 + 50 + self.rng.below(max));
        self.slots[s.0 as usize] = Slot::Down { until: down };
        self.stats.crashes += 1;
        self.reset_site(s);
    }

    /// The host's record of a storage confirm that ended: a truncation caps what earlier confirms vouch for, and an
    /// append records the disk's new last index at the site's clock.
    fn note_synced(&mut self, s: SiteId, appended: bool) {
        let d = &self.disks[s.0 as usize].stored;
        let last = d.base.index.0 + d.log.len() as u64;
        let at = self.clock(s).0;
        let ring = &mut self.synced[s.0 as usize];
        for r in ring.iter_mut() {
            r.0 = r.0.min(last);
        }
        if appended {
            ring.push((last, at));
        }
    }

    /// The earliest time this site's host confirmed its log durable through `index`.
    fn synced_at(&self, s: SiteId, index: u64) -> Option<u64> {
        self.synced[s.0 as usize].iter().filter(|r| r.0 >= index).map(|r| r.1).min()
    }

    /// A reset loses the member's outstanding calls, its unconfirmed writes, and every output its gate still held,
    /// and opens its output gate.
    fn reset_site(&mut self, s: SiteId) {
        self.generation[s.0 as usize] += 1;
        self.gate[s.0 as usize] = Timestamp(0);
        self.pending_calls.retain(|_, pc| pc.caller != s);
        self.dep_reads.retain(|d| d.1 != s);
        let now = self.now;
        self.flights.retain(|f| f.env.from != s || f.left <= now);
        // A confirm that ended by now is durable even if `host_events` has not reported it yet.
        let (done, keep): (Vec<ConfirmEnd>, Vec<ConfirmEnd>) =
            std::mem::take(&mut self.confirm_ends).into_iter().partition(|c| c.1 == s && c.0 <= now);
        self.confirm_ends = keep.into_iter().filter(|c| c.1 != s).collect();
        let mut done = done;
        done.sort_by_key(|c| c.0);
        for c in done {
            for w in &c.4 {
                graftyard_raft::Storage::write(&mut self.disks[s.0 as usize], w);
            }
        }
        self.synced[s.0 as usize].clear();
    }

    pub fn crash_now(&mut self, s: SiteId, down_ms: u64) {
        self.slots[s.0 as usize] = Slot::Down { until: Timestamp(self.now.0 + down_ms) };
        self.stats.crashes += 1;
        self.reset_site(s);
    }

    fn restart(&mut self, s: SiteId) {
        self.restarts += 1;
        let seed = self.seed ^ self.restarts.wrapping_mul(0x51_7cc1_b727_220a) ^ u64::from(s.0);
        let now = self.clock(s);
        let mine = &self.disks[s.0 as usize];
        if let Some(led) = mine.stored.state.as_ref().and_then(|st| st.led_term) {
            let ghost = self.disks.iter().enumerate().any(|(k, d)| {
                k != s.0 as usize
                    && d.stored
                        .log
                        .iter()
                        .zip(&d.digests)
                        .any(|(e, dg)| e.term == led && mine.digests.get(e.index.0 as usize - 1) != Some(dg))
            });
            self.stats.ghosts += u64::from(ghost);
        }
        let (mut m, ready) =
            Member::restore(config(s), self.disks[s.0 as usize].stored.clone(), now, seed).expect("restore");
        m.set_log_window(SIM_WINDOW);
        self.checked[s.0 as usize] = 0;
        // Everything the restore read back is durable.
        let d = &self.disks[s.0 as usize].stored;
        self.synced[s.0 as usize] = vec![(d.base.index.0 + d.log.len() as u64, now.0)];
        let (_, out) = ready.persist(&mut self.disks[s.0 as usize]);
        self.slots[s.0 as usize] = Slot::Up(Box::new(m));
        let res = self.deliver_out(s, out);
        self.handle(s, res);
    }

    fn linked(&self, a: SiteId, b: SiteId) -> bool {
        match &self.partition {
            Some((side, _)) => side.contains(&a) == side.contains(&b),
            None => true,
        }
    }

    /// Reports ended storage confirms and the calls whose deadline passed with no reply.
    fn host_events(&mut self) {
        let now = self.now;
        let (mut ends, keep): (Vec<ConfirmEnd>, Vec<ConfirmEnd>) =
            std::mem::take(&mut self.confirm_ends).into_iter().partition(|e| e.0 <= now);
        self.confirm_ends = keep;
        ends.sort_by_key(|e| e.0);
        // Every confirm due by now is durable, even if reporting an earlier one crashes its member.
        for (_, s, generation, _, writes, _) in &ends {
            if *generation == self.generation[s.0 as usize] {
                for w in writes {
                    graftyard_raft::Storage::write(&mut self.disks[s.0 as usize], w);
                }
                self.note_synced(*s, writes.iter().any(|w| matches!(w, Write::Append(_))));
            }
        }
        for (_, s, generation, id, _, ack) in ends {
            // An earlier report in this loop may have crashed the member.
            if generation != self.generation[s.0 as usize] {
                continue;
            }
            if let Some((_, res)) = self.call(s, |m, t| m.confirm_end(t, id)) {
                self.handle(s, res);
            }
            if let Some(ack) = ack
                && generation == self.generation[s.0 as usize]
                && let Some((_, res)) = self.call(s, |m, t| m.self_ack(t, ack))
            {
                self.handle(s, res);
            }
        }
        for (s, g, c) in std::mem::take(&mut self.catchups) {
            if g == self.generation[s.0 as usize] {
                let res = self.serve_catchup(s, c);
                self.handle(s, res);
            }
        }
        let due: Vec<CallKey> = self.pending_calls.iter().filter(|(_, pc)| pc.due <= now).map(|(k, _)| *k).collect();
        for k in due {
            // An earlier report in this loop may have crashed the caller, which drops its pending calls.
            let Some(pc) = self.pending_calls.remove(&k) else { continue };
            if std::env::var("RAFT_TRACE_CALLS").is_ok() {
                eprintln!(
                    "{} call failed {:?} -> {:?} deadline {}",
                    self.now.0, pc.caller, pc.failure.to, pc.failure.deadline
                );
            }
            self.stats.call_failures += 1;
            if let Some((_, res)) = self.call(pc.caller, |m, t| m.call_failed(t, pc.failure)) {
                self.handle(pc.caller, res);
            }
        }
    }

    fn draw_gray(&mut self) {
        if !self.rng.chance(self.faults.gray) || self.site_faults.iter().any(|f| self.now < f.until) {
            return;
        }
        let site = SITES[self.rng.below(3) as usize];
        let kind = match self.rng.below(if self.faults.half_hung { 4 } else { 3 }) {
            0 => FaultKind::Calls(300),
            1 => FaultKind::SlowConfirm(300, 1_000),
            2 => FaultKind::FailWrites(300),
            _ => FaultKind::HalfHung(800),
        };
        // A half-hung site lasts 30 to 90 s, longer than any one timer of the protocol, so a stuck state shows as a
        // stretch past the liveness bound rather than ending with the fault.
        let until = match kind {
            FaultKind::HalfHung(_) => Timestamp(self.now.0 + 30_000 + self.rng.below(60_000)),
            _ => Timestamp(self.now.0 + 5_000 + self.rng.below(25_000)),
        };
        self.site_faults.push(SiteFault { site, from: self.now, until, kind });
        self.stats.gray_faults += 1;
    }

    /// One step: time moves, faults may start, due messages are delivered, members tick, clients act.
    pub fn step(&mut self, clients_active: bool) {
        self.stats.steps += 1;
        self.now = Timestamp(self.now.0 + 1 + self.rng.below(50));
        // Faults.
        if let Some((_, end)) = &self.partition
            && self.now >= *end
        {
            self.partition = None;
        }
        if self.partition.is_none() && self.rng.chance(self.faults.partition) {
            let lone = SITES[self.rng.below(3) as usize];
            let end = Timestamp(self.now.0 + 500 + self.rng.below(15_000));
            self.partition = Some((BTreeSet::from([lone]), end));
            self.stats.partitions += 1;
        }
        self.draw_gray();
        self.draw_moves();
        for s in SITES {
            let unconfirmed = self.confirm_ends.iter().any(|c| c.1 == s && c.0 > self.now);
            match &self.slots[s.0 as usize] {
                Slot::Down { until } if self.now >= *until => self.restart(s),
                Slot::Up(_) if self.rng.chance(self.faults.crash) => self.crash(s),
                Slot::Up(_) if unconfirmed && self.rng.chance(self.faults.crash_unconfirmed) => self.crash(s),
                _ => {}
            }
        }
        self.host_events();
        self.answer_deps();
        // Messages due now, in a seeded order.
        let mut due = Vec::new();
        let mut i = 0;
        while i < self.flights.len() {
            if self.flights[i].at <= self.now {
                due.push(self.flights.swap_remove(i));
            } else {
                i += 1;
            }
        }
        for k in (1..due.len()).rev() {
            let j = self.rng.below(k as u64 + 1) as usize;
            due.swap(k, j);
        }
        for f in due {
            let Envelope { from, to, msg } = f.env;
            if !self.linked(from, to) {
                self.stats.dropped += 1;
                continue;
            }
            // A reply ends the caller's pending call, unless the host already reported it failed.
            let probe = Envelope { from, to, msg };
            if let Some(k) = reply_key(&probe)
                && self.pending_calls.remove(&k).is_none()
                && matches!(probe.msg, Message::AppendRes(_) | Message::Pong { .. })
                && self.late_reply_dropped(&k)
            {
                continue;
            }
            let Envelope { from, to, msg } = probe;
            if std::env::var("RAFT_TRACE_VOTES").is_ok()
                && matches!(
                    msg,
                    Message::Vote(_)
                        | Message::VoteRes(_)
                        | Message::TimeoutNow { .. }
                        | Message::TransferRefused { .. }
                )
            {
                eprintln!("{} {from:?} -> {to:?} {msg:?}", self.now.0);
            }
            if let Message::Append(a) = &msg
                && !self.terms.iter().any(|t| t.0 == a.term)
            {
                self.terms.push((a.term, a.leader, self.now));
            }
            if let Some((_, res)) = self.call(to, |m, now| m.step(now, from, msg)) {
                self.handle(to, res);
            }
            self.check();
        }
        for s in SITES {
            if let Some((_, res)) = self.call(s, |m, now| m.tick(now)) {
                self.handle(s, res);
            }
        }
        if clients_active && self.rng.chance(2) {
            self.lease_id_block();
        }
        if clients_active && self.rng.chance(2) {
            self.record_events();
        }
        if clients_active {
            self.clients();
            if self.rng.chance(self.faults.hedge) {
                // A stray report about whoever the reporter last saw lead, from a random colo.
                let s = SITES[self.rng.below(3) as usize];
                let colo = Colo(self.rng.below(8) as u32);
                let term = self.member(s).map_or(Term(0), |m| m.term());
                let about = self.member(s).and_then(|m| m.leader()).unwrap_or(s);
                self.stats.hedges += 1;
                if let Some((_, res)) = self.call(s, |m, now| m.suspect(now, colo, about, term)) {
                    self.handle(s, res);
                }
            }
        }
        self.check();
        self.watch();
    }

    /// The host's own leadership moves: `/transfer` to a named voter, and a drain whose leader hands off to a target
    /// it picks. A drain ends after 2 to 20 s, and every member is told the drained set, as `SiteMode` does.
    fn draw_moves(&mut self) {
        if self.faults.transfer > 0
            && self.rng.chance(self.faults.transfer)
            && let Some(l) = self.leader()
        {
            let to = SITES[(l.0 as usize + 1 + self.rng.below(2) as usize) % 3];
            self.stats.transfers += 1;
            if let Some((_, res)) = self.call(l, |m, now| m.transfer(now, to)) {
                self.handle(l, res);
            }
        }
        if let Some((_, end)) = self.drained
            && self.now >= end
        {
            self.drained = None;
            self.tell_drained();
        }
        if self.faults.drain > 0 && self.drained.is_none() && self.rng.chance(self.faults.drain) {
            let site = SITES[self.rng.below(3) as usize];
            self.drained = Some((site, Timestamp(self.now.0 + 2_000 + self.rng.below(18_000))));
            self.stats.drains += 1;
            self.tell_drained();
        }
    }

    fn tell_drained(&mut self) {
        let set: Vec<SiteId> = self.drained.iter().map(|d| d.0).collect();
        for s in SITES {
            let set = set.clone();
            if let Some((_, res)) = self.call(s, |m, now| m.set_drained(now, set)) {
                self.handle(s, res);
            }
        }
    }

    /// Voters that are up, under no gray fault, and linked to each other: the most of them in one linked set.
    pub fn healthy(&self) -> Vec<SiteId> {
        let ok: Vec<SiteId> =
            SITES.iter().copied().filter(|s| self.member(*s).is_some() && self.fault_on(*s).is_none()).collect();
        let mut best: Vec<SiteId> = Vec::new();
        for a in &ok {
            let side: Vec<SiteId> = ok.iter().copied().filter(|b| self.linked(*a, *b)).collect();
            if side.len() > best.len() {
                best = side;
            }
        }
        best
    }

    /// A member of `among` that leads and has committed an entry of its own term, so it serves.
    pub fn serving(&self, among: &[SiteId]) -> Option<SiteId> {
        among.iter().copied().find(|s| {
            self.member(*s).is_some_and(|m| {
                m.role() == Role::Leader && m.log().iter().any(|e| e.term == m.term() && e.index <= m.state().commit)
            })
        })
    }

    /// The liveness monitor (F67), for the property in `tests/m0/results/f67.md`: a stretch over which the same two or
    /// more voters stayed up, linked, and fault-free (`healthy`), a client request was waiting, and no request
    /// completed. A change in that set starts a new stretch, since the property assumes it holds. Recorded when it
    /// ends.
    fn watch(&mut self) {
        let h = self.healthy();
        let waiting = self.ops.iter().any(Option::is_some);
        let cond = h.len() >= 2 && waiting;
        if self.stuck_set != h {
            self.progressed();
            self.stuck_set = h;
        }
        match (cond, self.stuck_since) {
            (true, None) => self.stuck_since = Some(self.now),
            (false, Some(_)) => self.progressed(),
            _ => {}
        }
    }

    /// A request completed, or the condition ended: the open stretch ends.
    fn progressed(&mut self) {
        if let Some(from) = self.stuck_since.take() {
            self.leaderless.push((from, self.now));
        }
    }

    /// The longest stretch the monitor saw, including one still open, in ms, and when it started.
    pub fn longest_leaderless(&self) -> (u64, Timestamp) {
        let open = self.stuck_since.map(|f| (f, self.now));
        self.leaderless.iter().copied().chain(open).map(|(f, t)| (t.0 - f.0, f)).max().unwrap_or((0, Timestamp(0)))
    }

    /// The host recorded a block the `Ids` group leased to this group: usually the next one, sometimes one it already
    /// recorded, as a host that lost a reply would.
    fn lease_id_block(&mut self) {
        let Some(s) = self.leader() else { return };
        let b = if self.next_block > 1 && self.rng.chance(200) {
            self.next_block - 1 - self.rng.below(u64::from(self.next_block - 1).min(3)) as u32
        } else {
            self.next_block += 1;
            self.next_block - 1
        };
        let block = graftyard_raft::IdBlock::whole(0, b).expect("block fits");
        self.stats.id_blocks += 1;
        if let Some((_, res)) = self.call(s, |m, now| m.id_block(now, block)) {
            self.handle(s, res);
        }
    }

    /// The leader's host records events no command decided, as a repository leader records `Promoted` and
    /// `Archived`: an append to `REC_KEY`, which clients never use.
    fn record_events(&mut self) {
        let Some(s) = self.leader() else { return };
        // Usually a new record; sometimes one recorded already, as a host whose entry may have been lost re-sends it.
        // Its request row keeps it from applying twice, which the register would show as a repeated `n,`.
        let n = if self.stats.records > 0 && self.rng.chance(200) {
            self.stats.records - self.rng.below(self.stats.records.min(3))
        } else {
            self.stats.records += 1;
            self.stats.records
        };
        let id = RequestId(graftyard_raft::Ulid((0xEC_u128 << 120) | u128::from(n)));
        let ev = KvEvent::Push { key: super::kv::REC_KEY.into(), s: format!("{n},") };
        if let Some((_, res)) = self.call(s, |m, now| m.record(now, id, KV_FORMAT, vec![ev])) {
            self.handle(s, res);
        }
    }

    /// A reply that arrives after the host reported its call failed is still delivered half the time: a host may
    /// pass a late reply to `step` (`Member::call_failed`).
    fn late_reply_dropped(&mut self, _k: &CallKey) -> bool {
        self.rng.chance(500)
    }

    pub fn run(&mut self, steps: u64) {
        for _ in 0..steps {
            self.step(true);
        }
    }

    /// Lets every operation finish with no new ones and no faults.
    pub fn settle(&mut self, max_steps: u64) {
        self.faults = Faults::NONE;
        self.partition = None;
        for _ in 0..max_steps {
            if self.ops.iter().all(Option::is_none) {
                break;
            }
            self.step(false);
            self.drive_ops();
        }
    }

    // ---- clients ----

    fn clients(&mut self) {
        for c in 0..self.ops.len() {
            if self.ops[c].is_none() && self.rng.chance(150) {
                self.start_op(c);
            }
        }
        self.drive_ops();
    }

    fn start_op(&mut self, client: usize) {
        let key = format!("k{}", self.rng.below(self.keys as u64));
        self.next_value += 1;
        let value = format!("{client}.{}", self.next_value);
        let op = match if self.edge.reads_only { 0 } else { self.rng.below(10) } {
            0..=3 => HOp::Get { key },
            4..=6 => HOp::Put { key, value },
            _ => {
                let expect = self.last_seen(&key);
                HOp::Cas { key, expect, value }
            }
        };
        self.next_id += 1;
        let target = SITES[self.rng.below(3) as usize];
        let id = RequestId(graftyard_raft::Ulid(self.next_id));
        let colo = Colo(client as u32);
        let receipt = command(&op).map(|cmd| Receipt::new(id, cmd, target, colo));
        self.ops[client] = Some(Op {
            client,
            op,
            id,
            call: self.now,
            target,
            wait: None,
            retry_at: self.now,
            deadline: Timestamp(self.now.0 + 30_000),
            missed: false,
            term: Term(0),
            receipt,
            spread: false,
        });
    }

    /// A plausible `expect` for a compare-and-set: a value some member holds.
    fn last_seen(&mut self, key: &str) -> Option<String> {
        let s = SITES[self.rng.below(3) as usize];
        self.member(s).and_then(|m| m.group_fold().fold.0.get(key).cloned())
    }

    fn drive_ops(&mut self) {
        for c in 0..self.ops.len() {
            let Some(op) = &self.ops[c] else { continue };
            if self.now >= op.deadline {
                let op = self.ops[c].take().expect("op");
                self.outstanding.remove(&op.id);
                self.finish(op, None);
                self.stats.gave_up += 1;
                continue;
            }
            if self.now < op.retry_at {
                continue;
            }
            if let Some(Wait::Later(s, g)) = &op.wait {
                let (s, g) = (*s, g.clone());
                // A member that crashed meanwhile lost the waiter with its object: the edge sees a shed read.
                let v = self.member(s).map(|m| {
                    let HOp::Get { key } = &op.op else { unreachable!() };
                    m.fresh_after(&g).map(|f| f.fold().0.get(key).cloned())
                });
                let op = self.ops[c].take().expect("op");
                match v {
                    Some(Ok(v)) => {
                        self.stats.reads += 1;
                        self.stats.spread_reads += 1;
                        self.finish(op, Some(HOut::Value(v)));
                        self.progressed();
                    }
                    _ => {
                        self.stats.shed += 1;
                        self.finish(op, None);
                    }
                }
                continue;
            }
            if matches!(op.wait, Some(Wait::Spread(..))) {
                // The follower did not answer within FOLLOWER_WAIT: `overloaded`, a 503, no `suspect`.
                let op = self.ops[c].take().expect("op");
                self.stats.shed += 1;
                self.finish(op, None);
                continue;
            }
            self.send_op(c);
        }
    }

    /// The edge's receipt deadline: COMMIT_HOLD + CALL_DEADLINE for a held `commit`, CALL_DEADLINE for a read.
    fn receipt_wait(&self, write: bool) -> u64 {
        let d = graftyard_raft::CALL_DEADLINE.as_millis() as u64;
        if write { d + graftyard_raft::COMMIT_HOLD.as_millis() as u64 } else { d }
    }

    /// (Re)sends the client's operation to its target, or polls its ticket. A receipt that missed its deadline is
    /// hedged first: `suspect` reports to the other members, and the command goes to the same target again.
    fn send_op(&mut self, c: usize) {
        // `read_spread` asks followers first and never the member it last saw lead, live or not.
        let first_spread = self.ops[c].as_ref().is_some_and(|o| matches!(o.op, HOp::Get { .. }) && !o.spread);
        if self.edge.follower_reads && first_spread {
            let fails = self.rng.chance(self.faults.drop);
            self.spread_read(c, fails);
            return;
        }
        let op = self.ops[c].as_ref().expect("op");
        let (target, id, wait, missed) = (op.target, op.id, op.wait.clone(), op.missed);
        let hop = op.op.clone();
        let write = op.receipt.is_some();
        let retry = Timestamp(self.now.0 + self.receipt_wait(write));
        if missed {
            self.stats.missed += 1;
            self.ops[c].as_mut().expect("op").missed = false;
            self.hedge(c);
            if self.ops[c].is_none() {
                return;
            }
        }
        let target = self.ops[c].as_ref().map_or(target, |o| o.target);
        if self.member(target).is_none() {
            // A call to a resetting object hangs until its deadline: a missed receipt.
            let o = self.ops[c].as_mut().expect("op");
            o.missed = true;
            o.retry_at = Timestamp(self.now.0 + graftyard_raft::CALL_DEADLINE.as_millis() as u64);
            return;
        }
        if let Some(Wait::Ticket(t)) = wait {
            // A held ticket past its deadline: ask by request id, then re-send with the same id.
            let cmd = command(&hop).expect("write");
            let by = self.member(target).expect("up").outcome_by_request(id, &cmd);
            match by {
                Ok(ByRequest::Done(a)) => {
                    let ack = self.ack_evidence(target, &a);
                    let op = self.ops[c].take().expect("op");
                    self.outstanding.remove(&op.id);
                    self.stats.replays += 1;
                    self.write_done(target, op, Ok((a.index, a.out)), Some(ack));
                    return;
                }
                Ok(ByRequest::Pending(t2)) if t2 == t => {
                    let o = self.ops[c].as_mut().expect("op");
                    o.retry_at = retry;
                    o.missed = true;
                    return;
                }
                _ => {}
            }
        }
        {
            let o = self.ops[c].as_mut().expect("op");
            o.retry_at = retry;
            // Until an answer arrives, the next due time is a missed receipt.
            o.missed = true;
        }
        let fails = self.call_fails(target, target) || self.rng.chance(self.faults.drop);
        match &hop {
            HOp::Get { .. } => {
                if fails {
                    return;
                }
                let Some((r, res)) = self.call(target, |m, now| m.read(now)) else { return };
                match r {
                    Ok(rid) => {
                        // Every member reply carries `ReplyMeta`, so the edge knows the leader's term.
                        let term = self.member(target).map_or(Term(0), |m| m.meta().term);
                        let o = self.ops[c].as_mut().expect("op");
                        o.wait = Some(Wait::Read(rid));
                        o.term = o.term.max(term);
                    }
                    Err(nl) => self.not_leader(c, &nl),
                }
                self.handle(target, res);
            }
            _ => {
                // BF-14: the command goes only to the receipt's target; an outstanding receipt at another member
                // would be a forwarded command.
                let (to, rid, cmd) = {
                    let r = self.ops[c].as_ref().expect("op").receipt.as_ref().expect("write");
                    let (to, rid, cmd) = r.call();
                    (to, rid, cmd.clone())
                };
                if let Some((other, until)) = self.outstanding.get(&rid)
                    && *other != to
                    && self.now < *until
                {
                    self.stats.forwarded += 1;
                }
                self.outstanding.insert(rid, (to, retry));
                if fails {
                    return;
                }
                let Some((r, res)) = self.call(to, |m, now| m.commit(now, rid, cmd)) else { return };
                match r {
                    Ok(Admitted { ticket, .. }) => {
                        let now = self.now;
                        self.admitted_at.entry((to, ticket.term, ticket.boot, rid)).or_insert(now);
                        let o = self.ops[c].as_mut().expect("op");
                        o.wait = Some(Wait::Ticket(ticket));
                        o.term = o.term.max(ticket.term);
                        if let Some(r) = o.receipt.as_mut() {
                            r.saw_term(ticket.term);
                        }
                        // A ticket that already resolved (a duplicate answered from its row) is answered by
                        // `outcome` now.
                        if let Some(o) = self.member(to).and_then(|m| m.outcome(ticket)) {
                            self.ticket_done(to, ticket, o);
                        }
                    }
                    Err(CallError::NotLeader(nl)) => self.not_leader(c, &nl),
                    Err(e) => panic!("unexpected {e:?}"),
                }
                self.handle(to, res);
            }
        }
    }

    /// `read_spread`: the get goes by read-index to a voter other than the one the edge last saw lead. A `NotLeader`
    /// sends it on to the leader's path (`call_opt`, which reports with `suspect`); no answer within FOLLOWER_WAIT is
    /// a shed read, a 503 with no report.
    fn spread_read(&mut self, c: usize, fails: bool) {
        let o = self.ops[c].as_mut().expect("op");
        o.spread = true;
        o.missed = false;
        // `spread_order` leaves out the leader the edge last learned: the newest term's leader it saw.
        let not = self.terms.last().map_or(o.target, |t| t.1);
        let to = SITES[(not.0 as usize + 1 + self.rng.below(2) as usize) % 3];
        let (now, down) = (self.now, self.member(to).is_none());
        let o = self.ops[c].as_mut().expect("op");
        o.retry_at = Timestamp(now.0 + FOLLOWER_WAIT_MS);
        if fails || down {
            o.wait = Some(Wait::Spread(to, ReadId(u64::MAX)));
            return;
        }
        let Some((r, res)) = self.call(to, |m, now| m.read_index(now)) else { return };
        match r {
            Ok(rid) => self.ops[c].as_mut().expect("op").wait = Some(Wait::Spread(to, rid)),
            Err(_) => {
                // Declined: the leader's path, at once.
                let o = self.ops[c].as_mut().expect("op");
                o.wait = None;
                o.retry_at = self.now;
            }
        }
        self.handle(to, res);
    }

    fn config_of(&self) -> graftyard_raft::GroupConfig {
        SITES
            .iter()
            .find_map(|s| self.member(*s).and_then(|m| m.committed_config().map(|c| c.1.clone())))
            .unwrap_or_else(|| match graftyard_raft::bootstrap::<KvEvent>(CHAIN, &record()).expect("record").body {
                EntryBody::Config(c) => c,
                _ => unreachable!(),
            })
    }

    /// The hedge on a missed receipt: a `suspect` report to each other member. It carries no command.
    fn hedge(&mut self, c: usize) {
        let op = self.ops[c].as_ref().expect("op");
        let (target, term, client) = (op.target, op.term, op.client);
        let config = self.config_of();
        let reports: Vec<graftyard_raft::Suspect> = match &op.receipt {
            Some(r) => r.missed(&config).into_vec(),
            None => graftyard_raft::quorums::voters(&config)
                .into_iter()
                .filter(|s| *s != target)
                .map(|s| graftyard_raft::Suspect { to: s, from: Colo(client as u32), about: target, term })
                .collect(),
        };
        self.send_suspects(c, reports);
        // A read is not a command, so it may move to a member that now leads.
    }

    fn send_suspects(&mut self, c: usize, reports: Vec<graftyard_raft::Suspect>) {
        let client = self.ops[c].as_ref().map_or(c, |o| o.client);
        let gap = self.edge.suspect_gap_ms;
        let all = reports.iter().any(|r| r.to == r.about);
        if gap > 0
            && let Some(r) = reports.first()
        {
            let key = (client, r.about, r.term, all);
            if self.reported.get(&key).is_some_and(|t| self.now.0 < t.0 + gap) {
                self.stats.damped += reports.len() as u64;
                return;
            }
            self.reported.insert(key, self.now);
        }
        for r in reports {
            if self.call_fails(r.to, r.to) {
                continue;
            }
            self.stats.suspects += 1;
            let Some((v, res)) = self.call(r.to, |m, now| m.suspect(now, r.from, r.about, r.term)) else { continue };
            if std::env::var("RAFT_TRACE").is_ok() {
                eprintln!("{} suspect to {:?} from {:?} term {:?}: {v:?}", self.now.0, r.to, r.from, r.term);
            }
            self.handle(r.to, res);
            if let Some(op) = self.ops[c].as_mut() {
                if let Some(rc) = op.receipt.as_mut() {
                    rc.verdict(r.to, v);
                    op.target = rc.target();
                } else if matches!(v, graftyard_raft::Verdict::Leader | graftyard_raft::Verdict::Won) {
                    op.target = r.to;
                }
            }
        }
    }

    /// A member answered `NotLeader`: follow a named leader, or report `suspect` to every member and move on.
    fn not_leader(&mut self, c: usize, nl: &graftyard_raft::NotLeader) {
        if std::env::var("RAFT_TRACE").is_ok() {
            eprintln!("{} op {c} NotLeader {:?} {:?}", self.now.0, nl.term, nl.leader);
        }
        let config = nl.config.as_ref().map(|x| x.1.clone()).unwrap_or_else(|| self.config_of());
        let op = self.ops[c].as_mut().expect("op");
        op.wait = None;
        op.missed = false;
        op.term = op.term.max(nl.term);
        op.retry_at = self.now;
        let reports = match op.receipt.as_mut() {
            Some(r) => {
                let reports = r.not_leader(nl, &config).into_vec();
                op.target = r.target();
                reports
            }
            None => {
                let old = op.target;
                match nl.leader {
                    Some(l) if l != old => {
                        op.target = l;
                        Vec::new()
                    }
                    _ => {
                        let (term, client) = (op.term, op.client);
                        op.target = SITES[(old.0 as usize + 1) % 3];
                        SITES
                            .iter()
                            .map(|s| graftyard_raft::Suspect { to: *s, from: Colo(client as u32), about: old, term })
                            .collect()
                    }
                }
            }
        };
        let id = op.id;
        self.outstanding.remove(&id);
        self.send_suspects(c, reports);
    }

    fn handle(&mut self, s: SiteId, res: Vec<Resolution<KvCmd>>) {
        for r in res {
            match r {
                Resolution::Ticket { ticket, result } => self.ticket_done(s, ticket, result),
                Resolution::Read { id, result } => self.read_done(s, id, result),
                Resolution::Campaign(_) => {}
                Resolution::IdBlock { block, result: Ok(index) } => {
                    self.check_committed_at(s, index.0, "IdBlock");
                    let held = self.held(s, index.0).map(|h| h.1);
                    assert_eq!(
                        held,
                        Some(EntryBody::IdBlock(block)),
                        "seed {}: IdBlock resolved at {index:?}",
                        self.seed
                    );
                }
                Resolution::IdBlock { result: Err(_), .. } => {}
                Resolution::Recorded { id, result: Ok(index) } => {
                    self.check_committed_at(s, index.0, "Recorded");
                    let n = id.0.0 & u128::from(u64::MAX);
                    let held = self.held(s, index.0).map(|h| h.1);
                    let want = KvEvent::Push { key: super::kv::REC_KEY.into(), s: format!("{n},") };
                    assert_eq!(
                        held,
                        Some(EntryBody::Events(vec![want])),
                        "seed {}: Recorded({n}) resolved at {index:?}",
                        self.seed
                    );
                    self.stats.records_committed += 1;
                }
                Resolution::Recorded { result: Err(_), .. } => {}
            }
        }
    }

    fn ticket_done(
        &mut self,
        s: SiteId,
        ticket: Ticket,
        result: Result<graftyard_raft::Acked<bool>, CommitError<super::kv::KvErr>>,
    ) {
        // Every resolution, of any client, must rest on a committed entry, and every `Acked` on its evidence.
        if let Ok(a) = &result {
            self.ack_evidence(s, a);
            self.check_committed_at(s, a.index.0, "Acked");
        }
        if let Err(CommitError::Refused { index, err }) = &result {
            self.check_committed_at(s, index.0, "refusal");
            assert_eq!(*err, KvErr::Stale, "seed {}: a put decided without exactly its dependency", self.seed);
        }
        if std::env::var("RAFT_TRACE").is_ok() {
            eprintln!("{} ticket {ticket:?} at {s:?}: {:?}", self.now.0, result.as_ref().map(|a| a.index));
        }
        let Some(c) = self.ops.iter().position(|o| {
            o.as_ref().is_some_and(|o| o.target == s && matches!(o.wait, Some(Wait::Ticket(t)) if t == ticket))
        }) else {
            return;
        };
        match result {
            Ok(a) => {
                let ack = self.ack_evidence(s, &a);
                let op = self.ops[c].take().expect("op");
                self.outstanding.remove(&op.id);
                let d = self.held(s, a.index.0).expect("acked entry is held").0;
                let t = self.member(s).expect("up").term();
                self.acked.push((op.id, a.index.0, d, t));
                self.write_done(s, op, Ok((a.index, a.out)), Some(ack));
            }
            Err(CommitError::Refused { index, .. }) => {
                let op = self.ops[c].take().expect("op");
                self.outstanding.remove(&op.id);
                self.write_done(s, op, Err(index), None);
            }
            Err(CommitError::NotLeader(nl)) => self.not_leader(c, &nl),
            Err(CommitError::Full { .. }) => panic!("seed {}: a Human command was refused by admission", self.seed),
        }
    }

    fn check_committed_at(&self, s: SiteId, index: u64, what: &str) {
        let m = self.member(s).expect("up");
        assert!(
            m.state().commit.0 >= index,
            "seed {}: {what} at {index} from {s:?} whose commit is {:?}",
            self.seed,
            m.state().commit
        );
    }

    /// The answering member's evidence for an `Acked` it answers now, as a host renders it, after the sim checks it
    /// against what only the sim knows: each counted site's disk holds the leader's entry at the proof's index.
    fn ack_evidence(&self, s: SiteId, a: &graftyard_raft::Acked<bool>) -> Ack {
        let seed = self.seed;
        let p = a.quorum();
        let m = self.member(s).expect("up");
        let ack = Ack {
            site: s.0,
            index: a.index.0,
            commit_index: p.index().0,
            entry_at: p.entry_at().0,
            commit_at: p.at().0,
            sent_at: self.clock(s).0,
            self_synced_at: self.synced_at(s, p.index().0),
            acks: p.acks().iter().map(|d| (d.site.0, d.durable.0, d.at.0)).collect(),
        };
        if let Err(e) = super::history::ack_check(&ack, 2) {
            panic!("seed {seed}: {s:?} answered {:?} with bad evidence: {e} ({ack:?})", a.index);
        }
        let want = self.held(s, p.index().0).expect("the proof's entry is held").0;
        for d in p.acks() {
            let on_disk = self.disks[d.site.0 as usize].digests.get(p.index().0 as usize - 1);
            assert_eq!(
                on_disk,
                Some(&want),
                "seed {seed}: {s:?} counted {:?} toward {:?}, whose disk does not hold it ({ack:?})",
                d.site,
                p.index()
            );
        }
        assert!(
            graftyard_raft::quorums::commit(m.latest_config(), &p.sites().iter().copied().collect()),
            "seed {seed}: {s:?}'s proof for {:?} names no quorum ({ack:?})",
            p.index()
        );
        ack
    }

    fn write_done(&mut self, s: SiteId, op: Op, r: Result<(Index, bool), Index>, ack: Option<Ack>) {
        match r {
            Ok(_) => {
                self.stats.acked += 1;
                self.acks.push((self.now, s));
            }
            Err(_) => self.stats.refused += 1,
        }
        self.progressed();
        let ok = r.as_ref().is_ok_and(|(_, o)| *o);
        self.finish_with(op, Some(HOut::Ok(ok)), ack);
    }

    fn read_done(
        &mut self,
        s: SiteId,
        id: ReadId,
        result: Result<graftyard_raft::ReadGrant, graftyard_raft::NotLeader>,
    ) {
        let spread = self
            .ops
            .iter()
            .position(|o| o.as_ref().is_some_and(|o| matches!(o.wait, Some(Wait::Spread(f, r)) if f == s && r == id)));
        if let Some(c) = spread {
            match result {
                // Chosen by the read's id, not the run's generator, so every seed's other draws are unchanged.
                Ok(g) if later_ms(id).is_some() => {
                    // The answer is built later, within FOLLOWER_WAIT, without a new read-index round.
                    let later = Timestamp(self.now.0 + later_ms(id).expect("checked"));
                    let o = self.ops[c].as_mut().expect("op");
                    o.wait = Some(Wait::Later(s, g));
                    o.retry_at = later;
                }
                Ok(g) => {
                    let m = self.member(s).expect("up");
                    let fresh = m.fresh(&g).expect("grant used in the same step");
                    let HOp::Get { key } = &self.ops[c].as_ref().expect("op").op else { unreachable!() };
                    let v = fresh.fold().0.get(key).cloned();
                    let op = self.ops[c].take().expect("op");
                    self.stats.reads += 1;
                    self.stats.spread_reads += 1;
                    self.finish(op, Some(HOut::Value(v)));
                    self.progressed();
                }
                Err(_) => {
                    // Declined: the leader's path.
                    let o = self.ops[c].as_mut().expect("op");
                    o.wait = None;
                    o.retry_at = self.now;
                }
            }
            return;
        }
        let Some(c) = self.ops.iter().position(|o| {
            o.as_ref().is_some_and(|o| o.target == s && matches!(o.wait, Some(Wait::Read(r)) if r == id))
        }) else {
            return;
        };
        match result {
            Ok(g) => {
                let m = self.member(s).expect("up");
                let fresh = m.fresh(&g).expect("grant used in the same step");
                let HOp::Get { key } = &self.ops[c].as_ref().expect("op").op else { unreachable!() };
                let v = fresh.fold().0.get(key).cloned();
                let op = self.ops[c].take().expect("op");
                self.stats.reads += 1;
                self.finish(op, Some(HOut::Value(v)));
                self.progressed();
            }
            Err(nl) => self.not_leader(c, &nl),
        }
    }

    fn finish(&mut self, op: Op, out: Option<HOut>) {
        self.finish_with(op, out, None);
    }

    fn finish_with(&mut self, op: Op, out: Option<HOut>, ack: Option<Ack>) {
        self.history.push(Row { client: op.client, call: op.call.0, ret: self.now.0, op: op.op, out, ack });
    }

    // ---- invariants ----

    pub fn check(&mut self) {
        let seed = self.seed;
        if std::env::var("RAFT_TRACE").is_ok() {
            for s in SITES {
                let now = self.member(s).map(|m| (m.role(), m.term(), m.state().promise_until.0, m.state().transfer));
                let k = s.0 as usize;
                if now != self.traced[k] {
                    eprintln!("{} {s:?} -> {now:?}", self.now.0);
                    self.traced[k] = now;
                }
            }
        }
        for s in SITES {
            let Slot::Up(m) = &self.slots[s.0 as usize] else { continue };
            if m.role() == Role::Leader {
                let t = m.term();
                match self.leaders.get(&t) {
                    Some(l) => assert_eq!(*l, s, "seed {seed}: two leaders in term {t:?}"),
                    None => {
                        self.leaders.insert(t, s);
                        self.stats.leaders += 1;
                    }
                }
            }
            let commit = m.state().commit.0;
            assert!(commit <= m.last_index().0, "seed {seed}: {s:?} commit {commit} past its log");
            if m.role() == Role::Leader {
                // One entry in flight: past its `Noop`s, a leader holds at most one uncommitted entry of its term.
                let flight = m
                    .log()
                    .iter()
                    .filter(|e| e.index.0 > commit && e.term == m.term() && !matches!(e.body, EntryBody::Noop))
                    .count();
                assert!(flight <= 1, "seed {seed}: {s:?} has {flight} entries in flight");
            }
            self.stats.max_commit = self.stats.max_commit.max(commit);
            let base = m.log_base();
            assert!(base.index <= m.state().applied, "seed {seed}: {s:?} compacted past applied");
            if base.index.0 > 0 {
                self.stats.compacted += 1;
                let d = self.disks[s.0 as usize].digests.get(base.index.0 as usize - 1);
                assert!(
                    d.is_none_or(|d| *d == base.digest),
                    "seed {seed}: {s:?} base {:?} differs from its disk",
                    base.index
                );
            }
            // Entries at or below the base left memory: they were checked before, while held, or are on the disk.
            let from = self.checked[s.0 as usize].max(base.index.0);
            // The committed prefix of a live member never changes (truncating it asserts), so only new indices and
            // the uncommitted tail need checking.
            check_chain(seed, s, m, from);
            for i in from + 1..=commit {
                let d = m.digest_of(Index(i)).expect("held");
                let e = m.entry_at(Index(i)).expect("held");
                let t = e.term;
                match self.committed.get(&i) {
                    Some((d0, _)) => assert_eq!(*d0, d, "seed {seed}: {s:?} committed a different entry at {i}"),
                    None => {
                        self.committed.insert(i, (d, t));
                        if let EntryBody::IdBlock(b) = &e.body {
                            if let Some((at, prev)) = self.id_blocks.range(..i).next_back() {
                                assert!(
                                    prev.followed_by(*b),
                                    "seed {seed}: id block {b:?} at {i} rewinds {prev:?} at {at}"
                                );
                            }
                            self.id_blocks.insert(i, *b);
                            self.stats.id_blocks_committed += 1;
                        }
                    }
                }
                if let EntryBody::Events(_) = &e.body {
                    for (id, _) in &e.requests {
                        let first = self.applied_at.entry(*id).or_insert(i);
                        assert_eq!(*first, i, "seed {seed}: {s:?} applied request {id:?} at {first} and {i}");
                    }
                }
            }
            self.checked[s.0 as usize] = commit;
            assert_eq!(m.group_fold().applied(), m.state().commit, "seed {seed}: {s:?} applied lags commit");
            // The fold saw the latest committed block through `Fold::apply_id_block`.
            let latest = self.id_blocks.range(..=commit).next_back().map(|(_, b)| b.base().0.to_string());
            assert_eq!(
                m.group_fold().fold.0.get(super::kv::ID_KEY),
                latest.as_ref(),
                "seed {seed}: {s:?} fold misses the id block"
            );
            assert_eq!(m.group_fold().id_block().map(|b| b.base().0.to_string()), latest, "seed {seed}: {s:?}");
            // Each record applied once: a re-sent `Member::record` never appends its `n,` twice.
            if let Some(v) = m.group_fold().fold.0.get(super::kv::REC_KEY) {
                let mut seen = BTreeSet::new();
                assert!(
                    v.split_terminator(',').all(|n| seen.insert(n)),
                    "seed {seed}: {s:?} applied a record twice: {v}"
                );
            }
        }
        // Committed entries are never lost: each is on a quorum of disks, as the disks are what survive a crash.
        let max = self.committed.keys().next_back().copied().unwrap_or(0);
        for i in 1..=max {
            let (d, _) = self.committed[&i];
            let held = self.disks.iter().filter(|disk| disk.digests.get(i as usize - 1) == Some(&d)).count();
            assert!(held >= 2, "seed {seed}: committed index {i} is on {held} disks");
        }
        // Every Acked write is in the log of every leader of a later term.
        for s in SITES {
            let Some(m) = self.member(s) else { continue };
            if m.role() != Role::Leader {
                continue;
            }
            for (id, i, d, t) in &self.acked {
                // A leader of an older term, cut off and not yet deposed, may lack it; it can commit nothing.
                if m.term() < *t {
                    continue;
                }
                let held = if *i <= m.log_base().index.0 {
                    self.disks[s.0 as usize].digests.get(*i as usize - 1).copied()
                } else {
                    m.digest_of(Index(*i))
                };
                assert_eq!(
                    held.as_ref(),
                    Some(d),
                    "seed {seed}: leader {s:?} of {:?} lacks Acked {id:?} at {i} acked in {t:?}",
                    m.term()
                );
            }
        }
    }
}

/// The member's entries above `from` (at or above its base) chain to each other and to the one at `from`.
fn check_chain(seed: u64, s: SiteId, m: &Member<KvCmd>, from: u64) {
    let base = m.log_base().index.0;
    let mut prev = m.digest_of(Index(from)).expect("from is held");
    for (k, e) in m.log().iter().enumerate().skip((from - base) as usize) {
        assert_eq!(e.index.0, base + k as u64 + 1, "seed {seed}: {s:?} index gap");
        assert_eq!(e.prev, prev, "seed {seed}: {s:?} chain broken at {}", e.index.0);
        prev = m.digests()[k];
    }
}

pub fn command(op: &HOp) -> Option<KvCmd> {
    match op {
        HOp::Get { .. } => None,
        // Half the puts depend on another chain; the value's counter picks which, so a re-send sends the same command.
        HOp::Put { key, value } if value.ends_with(['1', '3', '5', '7', '9']) => {
            Some(KvCmd::PutDep { key: key.clone(), value: value.clone() })
        }
        HOp::Put { key, value } => Some(KvCmd::Put { key: key.clone(), value: value.clone() }),
        HOp::Cas { key, expect, value } => {
            Some(KvCmd::Cas { key: key.clone(), expect: expect.clone(), value: value.clone() })
        }
    }
}
