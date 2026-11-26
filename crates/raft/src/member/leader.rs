//! The leader: group commit, `decide`, replication, the commit-advance path, leases, read-index, and tickets.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use graftyard_types::{ChainId, IdBlock, SiteId, Timestamp};
use smallvec::SmallVec;

use crate::admit::{
    AGENT_CMDS_ENTRY, AGENT_FLOOR, AGENT_INLINE_ENTRY, AGENT_QUEUE_MAX, AGENT_SHARE, AGENT_WAIT, ActorKey, Admit,
    AdmitClass, AdmitError, DEMOTE_PER_MIN, DEMOTE_WINDOW, Drain, HOT_PUSHES, HOT_WINDOW, INLINE_MAX_HOT, Lane,
    PRESSURE_AGE, retry_after_s,
};
use crate::fold::{Command, Fold, Scratch};
use crate::outcome::{self, Acked};
use crate::quorum::{self, DurableAck, QuorumProof};
use crate::read::{Fresh, ReadGrant};
use crate::types::{
    Admitted, CallError, CommitError, EntryBody, ExtendLeases, Format, GroupConfig, Index, LogEntry, Next, NotLeader,
    RequestId, Term, Ticket, TierSet,
};
use crate::{CALL_DEADLINE, COMMAND_MAX, DEEP_QUEUE, DRIFT, ENTRY_MAX, ENTRY_SOFT_DEEP, LEASE, after};

use super::election::CAMPAIGN_DEADLINE;
use super::{
    AdmissionView, Alarm, AppendReq, AppendRes, ByRequest, CutEnd, CutNote, DepsUsed, DepsWanted, IdBlockError, Member,
    Message, ReadId, ReadStart, Ready, ReconfigError, Refusal, Resolution, Role, TransferWhy,
};

/// Entries per AppendEntries when a follower is behind.
const CATCHUP_MAX: usize = 64;
/// Encoded bytes per AppendEntries, catch-up or pipelined; one entry always goes, so an entry up to ENTRY_MAX still
/// replicates. A call to the far voter must answer within CALL_DEADLINE: at 1.5 MB, a far voter behind on calls of
/// 319 to 717 KB missed on every call, and B0-02's leader stepped down off its site (r330-6,
/// `tests/m0/results/carried-8.md`). Far calls on `Tri` took p99 810 to 869 ms at 100 to 125 KB, and
/// missed first at 150 KB; at 75 to 100 KB, p99 768 to 845 ms [measured, m0-carried-6 to -8], which leaves about 150 ms
/// of margin. A pipelined call shares the link with up to PIPELINE - 1 others, and calls of at most 125 KB with 500 to
/// 550 KB ahead of them still answered at p99 644 ms.
/// With 64 entries of up to ENTRY_MAX alone, 9 MB calls missed every time (`tests/m1/results/m1-wp8.md`, t-256).
pub const APPEND_BYTES_MAX: usize = 100_000;

/// How many of `entries` one AppendEntries carries: at most CATCHUP_MAX, and past the first, only while their encoded
/// size stays within APPEND_BYTES_MAX.
pub(super) fn append_batch<E: crate::fold::Record>(entries: &[LogEntry<E>]) -> usize {
    let mut bytes = 0;
    let mut n = 0;
    for e in entries.iter().take(CATCHUP_MAX) {
        bytes += crate::encode_entry(e).len();
        if n > 0 && bytes > APPEND_BYTES_MAX {
            break;
        }
        n += 1;
    }
    n
}
/// AppendEntries calls a leader keeps outstanding to one voter. A call to a voter that is caught up may leave while
/// earlier ones are still out, carrying only the entries after them, so a far voter whose call for entry k is still
/// running gets k+1 at once: with one call per voter, B0-02's far follower got each entry one whole call late, and in
/// four failing runs the near follower went quiet for 0.6 to 1.6 s while the far one, which then made the quorum,
/// waited on calls of 43 to 301 KB (`tests/m0/results/carried-8.md`). A voter whose link is lossy (F67 rule 13: over
/// `commit_errors` of every call in the `STEP_DOWN` window failed, at least `commit_fail_min`) gets one call at a time:
/// a half-hung leader whose few answered calls each let several more out renewed its followers' promises and held a
/// live-sweep group 50 s (seed 11497). One miss does not stop it: a far voter that missed once and lost its pipeline
/// for the window caught up one backlog call at a time, and each missed again (r330-6).
pub const PIPELINE: usize = 4;
/// The least time between two read-index rounds while the earlier one is unanswered (`Member::read_round`) [guess],
/// on the isolate's own clock (`Member::set_own_clock`): beside a follower's 10 ms read batching, it caps read rounds at
/// 50 a second per voter, whatever the floored member clock does.
pub const READ_ROUND_GAP: Duration = Duration::from_millis(20);
/// How old a voter's outstanding AppendEntries may be before a read round re-sends its entries rather than an empty
/// round (`Member::read_beat`) [guess]: half of CALL_DEADLINE, as a learner's HEDGE. Before read-beat every round
/// re-sent the entry in flight, which also re-sent a lost call within one round; past this a lost call is re-sent by
/// the next round, not CALL_DEADLINE's tick, and a call answered sooner is never sent twice.
pub const READ_RESEND: Duration = Duration::from_millis(500);
/// How often a follower in `cannot_follow` is sent to again.
const CANNOT_FOLLOW_RETRY: Duration = Duration::from_secs(5);
/// Resolved tickets kept for a late `outcome` call: at most DONE_KEEP of them, DONE_BYTES of the bytes their commands
/// carry that no event stores (`encoded_len` less `stored_len`, an issue command's reply render, which its outcome
/// keeps), and none past OUTCOME_KEEP. An `outcome` for a ticket pruned from here answers `NotLeader` naming this
/// leader, and the edge asks `outcome_by_request`, which answers a committed command from its `request` row. Kept by
/// count alone, 4,096 issue outcomes with their renders held about 60 MB of a leader's heap [measured, native
/// allocator, `tests/m0/results/memory-budgets.md`].
const DONE_KEEP: usize = 4096;
pub const DONE_BYTES: usize = 8_000_000;
/// The edge asks `outcome` within COMMIT_HOLD plus one call of a ticket's resolution, or after a missed 11 s
/// `/outcome` call [guess, with margin].
pub const OUTCOME_KEEP: Duration = Duration::from_secs(15);
/// AppendEntries bytes (their entries' canonical encoding) a leader keeps alive on its calls to one peer, to all its
/// learners together, and to every peer together, from the send until the call's answer, an error, or the host's
/// `call_ended` (`Member::calls_out`). A voter with no call alive always gets one, so the budget never stops a commit
/// [guess]. Before, nothing bounded the sum: the host awaits a missed call for LATE_LIMIT (10 s) while the core resends,
/// and new leaders held 58.3 MB of call bodies and 42.9 MB of messages held for learners before their isolates reset
/// (`tests/m0/results/memory-budgets.md`).
pub const PEER_OUT_MAX: usize = 2_000_000;
pub const LEARNERS_OUT_MAX: usize = 4_000_000;
pub const OUT_MAX: usize = 8_000_000;
/// A call the host never reports ended stops counting after this: past CALL_DEADLINE plus the host's 10 s LATE_LIMIT.
pub const OUT_KEEP: Duration = Duration::from_secs(12);

struct Queued<C> {
    seq: u64,
    id: RequestId,
    cmd: C,
    /// Its encoded length, and the Agent-class inline pack bytes it carries (0 for every other class).
    bytes: usize,
    agent_inline: usize,
    /// Its `stored_len` without its Agent inline pack bytes, which AGENT_INLINE_ENTRY bounds: what ENTRY_SOFT counts.
    soft_bytes: usize,
    /// AGENT_WAIT after its admission, or sooner by an Agent command's `Admit::budget`: still queued then, it is answered `Full`.
    until: Timestamp,
}

impl<C> Queued<C> {
    /// The bytes the command carries that no event stores: its encoded length less its `stored_len`.
    fn unstored(&self) -> usize {
        self.bytes.saturating_sub(self.soft_bytes + self.agent_inline)
    }
}

/// The leader's admission state (`crate::admit`): one lane per class, the agent token bucket, and the capacity
/// estimate it is sized from. Memory only; a new leader starts with a full bucket and reads `hot` and the
/// DEMOTE_PER_MIN counts from the fold.
struct Admission<C> {
    lanes: [Lane<Queued<C>>; 4],
    agent_bytes: usize,
    agent_inline: usize,
    tokens: f32,
    tokens_at: Timestamp,
    /// The commit round of an `Events` entry, and an admitted command's `stored_len` without its Agent inline pack, as
    /// moving averages.
    round_ms: f32,
    cmd_bytes: f32,
    /// When the last `Events` entry was cut.
    cut_at: Timestamp,
    /// A dependency read, `DepsWanted` to `deps_read`, as a moving average; `None` until this leadership measured
    /// one, and until then no batch is cut while an entry is in flight.
    deps_ms: Option<f32>,
    /// Commands this leadership admitted with an actor and has not resolved, per actor: they count toward
    /// DEMOTE_PER_MIN beside the committed ones in the fold.
    pending_by: BTreeMap<ActorKey, u32>,
    seq_actor: BTreeMap<u64, ActorKey>,
    /// Writes this leadership refused as Agent, per actor, by time: they never commit, so the fold cannot count them,
    /// and without them a demoted actor whose writes are refused falls back under DEMOTE_PER_MIN and is classed Human
    /// again. Memory only, so a new leader counts only what committed [inference].
    refused_by: BTreeMap<ActorKey, VecDeque<Timestamp>>,
    /// What this leadership's cuts drained of a non-empty agent lane (`admit::Drain`).
    drain: Drain,
}

/// The round a fresh leader assumes before it has measured one [guess: B0-01's commit p50 on `Tri`].
const ROUND_PRIOR_MS: f32 = 250.0;
/// No capacity estimate uses a round shorter than this [guess, the M0-WP9 prototype's floor].
const ROUND_MIN_MS: f32 = 50.0;
/// The most frozen batches a leader holds behind the entry in flight [guess]: B0-31's apac leader needs 2 (a 368 ms
/// read on a 198 ms round, `tests/am1/results/lm1-rows-m3f.md`, "B0-31's buckets-up commit tail"), and each one held
/// freezes its commands a round sooner than they would otherwise be.
pub const READ_AHEAD: usize = 3;

/// How many frozen batches the leader holds behind the entry in flight: enough that each batch's read, asked a round
/// after the one before, lands about when the entry before it commits, so the cycle is a round plus `decide` and not
/// a read plus `decide`. One while a read fits in a round, which is cutting early as before; at most READ_AHEAD.
/// How long an agent command admitted now should wait to enter an entry: the agent commands queued ahead of it at
/// `agent_rate` (`Admission::agent_rate`), plus one round for its own entry to commit.
fn expected_wait(agent_queued: usize, agent_rate: f32, round_ms: f32) -> Duration {
    Duration::from_millis((agent_queued as f32 / agent_rate.max(1.0) * 1000.0 + round_ms.max(ROUND_MIN_MS)) as u64)
}

pub fn read_ahead(round_ms: f32, deps_ms: f32) -> usize {
    ((deps_ms / round_ms.max(ROUND_MIN_MS)).ceil() as usize).clamp(1, READ_AHEAD)
}
/// Commands are priced at no fewer bytes than this, so a burst of tiny commands does not claim unbounded capacity
/// [guess, the M0-WP9 prototype's].
const CMD_BYTES_MIN: f32 = 200.0;
/// Actors whose refusals the leader keeps before it drops those with none in the last minute.
const REFUSED_ACTORS_MAX: usize = 20_000;

impl<C> Admission<C> {
    fn new(now: Timestamp) -> Admission<C> {
        Admission {
            lanes: Default::default(),
            agent_bytes: 0,
            agent_inline: 0,
            tokens: f32::MAX,
            tokens_at: now,
            round_ms: ROUND_PRIOR_MS,
            cmd_bytes: 1_000.0,
            cut_at: now,
            deps_ms: None,
            pending_by: BTreeMap::new(),
            seq_actor: BTreeMap::new(),
            refused_by: BTreeMap::new(),
            drain: Drain::default(),
        }
    }

    fn is_empty(&self) -> bool {
        self.lanes.iter().all(Lane::is_empty)
    }

    fn agent(&self) -> &Lane<Queued<C>> {
        &self.lanes[AdmitClass::Agent.lane()]
    }

    /// Commands a second: entries a second at the measured round, times the commands of the current byte mix one
    /// entry holds. `fixed` is the host's override (`Member::set_admit_capacity`).
    fn capacity(&self, fixed: Option<f32>, soft: usize) -> f32 {
        fixed.unwrap_or_else(|| {
            let per_entry = (soft as f32 / self.cmd_bytes.max(CMD_BYTES_MIN)).max(1.0);
            per_entry * 1000.0 / self.round_ms.max(ROUND_MIN_MS)
        })
    }

    /// Agent commands a second this leader drains: what its cuts took from a non-empty agent lane over DRAIN_WINDOW
    /// (`admit::Drain`), never above the AGENT_SHARE bucket's rate, which stands until DRAIN_MIN of it is measured.
    /// The bucket's rate counts commands an entry by bytes, but AGENT_CMDS_ENTRY and a slow host's rounds bound the
    /// drain below it: B0-33 pfA drained 54.6 a second at p50 while its bucket read about 1.3 times that.
    fn agent_rate(&self, now: Timestamp, cap: f32) -> f32 {
        let bucket = AGENT_SHARE * cap;
        self.drain.rate(now).map_or(bucket, |r| r.min(bucket)).max(1.0)
    }

    fn refill(&mut self, now: Timestamp, cap: f32) {
        let rate = AGENT_SHARE * cap;
        let dt = now.0.saturating_sub(self.tokens_at.0) as f32 / 1000.0;
        self.tokens = (self.tokens + rate * dt).min(rate.max(1.0));
        self.tokens_at = now;
    }

    fn take(&mut self, class: AdmitClass) -> Option<Queued<C>> {
        let q = self.lanes[class.lane()].pop()?;
        if class == AdmitClass::Agent {
            self.agent_bytes -= q.bytes;
            self.agent_inline -= q.agent_inline;
        }
        Some(q)
    }

    fn resolved(&mut self, seq: u64) {
        if let Some(a) = self.seq_actor.remove(&seq)
            && let Some(n) = self.pending_by.get_mut(&a)
        {
            *n -= 1;
            if *n == 0 {
                self.pending_by.remove(&a);
            }
        }
    }

    fn refused(&mut self, actor: Option<ActorKey>, now: Timestamp) {
        let Some(a) = actor else { return };
        let cut = Timestamp(now.0.saturating_sub(DEMOTE_WINDOW.as_millis() as u64));
        let q = self.refused_by.entry(a).or_default();
        q.push_back(now);
        while q.front().is_some_and(|t| *t < cut) {
            q.pop_front();
        }
        if self.refused_by.len() > REFUSED_ACTORS_MAX {
            self.refused_by.retain(|_, q| q.back().is_some_and(|t| *t >= cut));
        }
    }

    fn refused_since(&self, a: ActorKey, from: Timestamp) -> u32 {
        self.refused_by.get(&a).map_or(0, |q| q.iter().filter(|t| **t >= from).count() as u32)
    }

    fn drain(self) -> impl Iterator<Item = Queued<C>> {
        self.lanes.into_iter().flat_map(Lane::drain)
    }
}

/// A frozen batch of a later entry's commands and their dependency reads. Batches are cut while the entry before them
/// is in flight or held, so their reads overlap rounds (`cut_early`, up to READ_AHEAD of them), but each is decided only
/// once the entry before it committed and applied, in the order they were cut, on the fold then, with exactly the
/// chains `deps()` names on that fold: any it names that were not read yet are read then, and any read that it no
/// longer names are dropped.
struct Deciding<C: Command> {
    /// The outstanding `DepsWanted`, or `None` when no read is outstanding.
    id: Option<u64>,
    batch: Vec<Queued<C>>,
    /// Every chain a `DepsWanted` asked for this batch.
    chains: SmallVec<[ChainId; 3]>,
    /// The folds the host read, without the chains it could not read.
    deps: Vec<(ChainId, C::Dep)>,
    /// When the outstanding `DepsWanted` left, for `deps_ms`.
    asked: Timestamp,
    /// Every `DepsWanted` whose reads `deps` holds, for `DepsUsed`.
    ids: SmallVec<[u64; 2]>,
    /// `reask-read`: the batch was cut early with nothing left queued (`CutEnd::Empty`), so while the entry before
    /// it is in flight it takes the commands admitted since, each time a read asked after them answers. A cut that
    /// left a backlog (`Soft`, `EntryMax`, `AgentTokens`, `AgentCmds`) is never open, and a batch stops being open at
    /// its caps, when a later batch is cut behind it, at a transfer, and once the entry before it applied.
    open: bool,
    /// The entry in flight when the batch was cut: it takes commands only while that entry is still the one in flight.
    behind: Index,
    /// Agent commands in the batch, against AGENT_CMDS_ENTRY.
    agents: u32,
    /// The re-ask out for an open batch: its id, the last seq admitted when it was asked (every command it may bring
    /// in was admitted by then), its chains, and when it left. It never holds the batch: once the entry before it
    /// applies, an unanswered re-ask is dropped and the batch decides on the read it already has.
    reask: Option<(u64, u64, SmallVec<[ChainId; 3]>, Timestamp)>,
}

/// A decided command: its seq, the bytes its outcome keeps that no event stores (`Lead::done`), and its result.
type Decided<C> = (u64, usize, Result<<C as Command>::Out, <C as Command>::Err>);
type Outcome<C> = Result<Acked<<C as Command>::Out>, CommitError<<C as Command>::Err>>;

pub(super) struct Lead<C: Command> {
    started: Timestamp,
    /// The term this leadership issued its tickets in, which `step_down` may have raised before the leadership ends.
    term: Term,
    /// The leader serves once `commit` reaches this and a quorum answered a round sent at or after `started`: the
    /// term's `Noop`, or for a resumed `led_term` the last entry it held when it restarted.
    ready_at: Index,
    ready: bool,
    next: BTreeMap<SiteId, Index>,
    matched: BTreeMap<SiteId, Index>,
    /// Per follower, when `matched` last rose, on this leader's clock: the time of its `QuorumProof` ack.
    matched_at: BTreeMap<SiteId, Timestamp>,
    /// Per follower, the send time of the latest round it promised on, and of the latest round it answered.
    promised_at: BTreeMap<SiteId, Timestamp>,
    acked_at: BTreeMap<SiteId, Timestamp>,
    lease_end: Timestamp,
    /// The latest outstanding call to each follower.
    sent: BTreeMap<SiteId, (u64, Timestamp)>,
    /// Per voter, the outstanding AppendEntries a pipelined call may follow, oldest first: each round and the last
    /// index it carried. At most PIPELINE; cleared by any failure, refusal, catch-up, or install to the voter.
    pipe: BTreeMap<SiteId, Vec<(u64, Index)>>,
    round: u64,
    admission: Admission<C>,
    seq: u64,
    /// No commit below this index: the second `Noop` of a resumed `led_term` (`become_leader`), else 0.
    commit_floor: Index,
    /// Request ids queued, being decided, or in flight, so a re-send joins the ticket.
    by_request: BTreeMap<RequestId, u64>,
    /// Frozen batches, oldest first: the front is the next entry's.
    deciding: VecDeque<Deciding<C>>,
    /// The one entry in flight and its decided commands.
    pending: Option<(Index, Vec<Decided<C>>)>,
    /// Id blocks `Member::id_block` took, waiting for their own entry, in order.
    blocks: VecDeque<IdBlock>,
    /// The id block entry in flight, when `pending` is one.
    block_pending: Option<(Index, IdBlock)>,
    /// Events `Member::record` took, waiting for their own entry, in order, each under its request id.
    records: VecDeque<(RequestId, Format, Vec<crate::Ev<C>>)>,
    /// The recorded entry in flight, when `pending` is one.
    record_pending: Option<(Index, RequestId)>,
    /// Waiting reads: when each arrived, and whether the lease had lapsed then, so it needs a round.
    reads: Vec<(ReadId, Timestamp, bool)>,
    /// When the latest read round left (`read_round`), on the member clock, which a read's arrival is read against:
    /// a read that arrived at or before then shares it. And on the isolate's own clock, which the gap is read against.
    read_round: Option<(Timestamp, Timestamp)>,
    /// The latest commit proof this leadership counted.
    proof: Option<QuorumProof>,
    /// Resolved tickets for a late `outcome`, by seq: when each resolved, its priced bytes, and its result.
    done: BTreeMap<u64, (Timestamp, usize, Outcome<C>)>,
    done_bytes: usize,
    /// Replies to rounds below this are ignored: the leader truncated its uncommitted tail after they were sent, so
    /// their `matched` may name entries it no longer holds.
    pub(super) min_round: u64,
    /// Per address whose member changed in this leadership (`take_config`), the first round sent to the new member:
    /// a reply to an earlier round came from the old member there and moves nothing for the new one.
    member_from: BTreeMap<SiteId, u64>,
    /// The transfer in progress.
    pub(super) transfer: Option<super::transfer::Transfer>,
    /// `STEP_DOWN`'s inputs and state.
    pub(super) health: super::health::LeaderHealth,
    /// `verify` refusals waiting to be classified: the refuser, its refusal, and when it first arrived.
    refusals: BTreeMap<SiteId, (Refusal, Timestamp)>,
    /// Promises from rounds sent before this extend no lease: an abandoned transfer reset it.
    lease_floor: Timestamp,
    /// The follower an older-release refusal came from: the preferred target of the `no_lead_until` hand-off.
    pub(super) refuser: Option<SiteId>,
    /// Per follower, the failed calls since its last answer: a commit-path call retries at once at most
    /// COMMIT_RETRIES times, then waits for `tick`.
    retries: BTreeMap<SiteId, u8>,
    /// Since when this leader has had work that needs a quorum's answer (an entry in flight, a read, or a transfer).
    waiting_since: Option<Timestamp>,
    /// Per follower, when an AppendEntries answer of this term last arrived, on this leader's clock. Unlike
    /// `acked_at`, a transfer's lease reset does not clear it.
    heard: BTreeMap<SiteId, Timestamp>,
    /// Per follower, the commit index it acknowledged: the lower of the commit an answered round carried and the index
    /// it matched. `tick` sends a follower below the leader's commit one round once its last round is CALL_DEADLINE old,
    /// so a group that commits and goes quiet still tells its followers (R29, `r29-commit-heartbeat`).
    told: BTreeMap<SiteId, Index>,
    /// Per follower, the latest AppendEntries round sent, the commit it carried, and when.
    telling: BTreeMap<SiteId, (u64, Index, Timestamp)>,
}

impl<C: Command> Lead<C> {
    pub(super) fn matched(&self, s: SiteId) -> Index {
        self.matched.get(&s).copied().unwrap_or(Index(0))
    }
    pub(super) fn started(&self) -> Timestamp {
        self.started
    }
    pub(super) fn ready(&self) -> bool {
        self.ready
    }
    pub(super) fn round(&self) -> u64 {
        self.round
    }
    /// No lease, and no read-index round, until a quorum answers a round sent from now. A transfer vote may have
    /// elected the target since any earlier round was answered.
    pub(super) fn reset_lease(&mut self, now: Timestamp) {
        self.lease_end = Timestamp(0);
        self.promised_at.clear();
        self.acked_at.clear();
        self.lease_floor = now;
    }
}

impl<C: Command> Member<C> {
    /// The other voters: who votes, counts toward a lease or a commit, may take leadership, and whose calls
    /// `STEP_DOWN` weighs.
    pub(super) fn peers(&self) -> Vec<SiteId> {
        quorum::voters(self.log_config()).into_iter().filter(|s| *s != self.cfg.me).collect()
    }

    /// Everyone the leader replicates to: the other voters and every learner, by address. Learners never count.
    pub(super) fn targets(&self) -> Vec<SiteId> {
        let c = self.log_config();
        let mut t = quorum::voters(c);
        t.extend(quorum::learners(c));
        t.into_iter().filter(|s| *s != self.cfg.me).collect()
    }

    fn ticket(&self, seq: u64) -> Ticket {
        Ticket { term: self.state.term, boot: self.boot, seq }
    }

    pub(super) fn become_leader(&mut self, resumed: bool) {
        self.role = super::Role::Leader;
        self.leader = Some(self.cfg.me);
        self.campaign = None;
        self.state.led_term = Some(self.state.term);
        self.state.lead_seq += 1;
        self.dirty = true;
        let next = self.last().next();
        let peers = self.targets();
        self.early_from = Index(0);
        self.lead = Some(Lead {
            started: self.now,
            term: self.state.term,
            ready_at: Index(0),
            ready: false,
            next: peers.iter().map(|p| (*p, next)).collect(),
            matched: peers.iter().map(|p| (*p, Index(0))).collect(),
            matched_at: BTreeMap::new(),
            promised_at: BTreeMap::new(),
            acked_at: BTreeMap::new(),
            lease_end: Timestamp(0),
            sent: BTreeMap::new(),
            pipe: BTreeMap::new(),
            // Rounds never repeat within a boot, so `calls_out` keys an earlier leadership's calls apart.
            round: self.lead.as_ref().map_or(self.rounds, |l| l.round).max(self.rounds),
            admission: Admission::new(self.now),
            // Seqs never repeat within a boot either: a ticket names (term, boot, seq), and a resumed term keeps both.
            seq: self.lead.as_ref().map_or(self.seqs, |l| l.seq).max(self.seqs),
            commit_floor: Index(0),
            by_request: BTreeMap::new(),
            deciding: VecDeque::new(),
            pending: None,
            blocks: VecDeque::new(),
            block_pending: None,
            records: VecDeque::new(),
            record_pending: None,
            reads: Vec::new(),
            read_round: None,
            proof: None,
            done: BTreeMap::new(),
            done_bytes: 0,
            min_round: 0,
            member_from: BTreeMap::new(),
            transfer: None,
            health: super::health::LeaderHealth::default(),
            refusals: BTreeMap::new(),
            lease_floor: Timestamp(0),
            refuser: None,
            retries: BTreeMap::new(),
            waiting_since: None,
            heard: BTreeMap::new(),
            told: BTreeMap::new(),
            telling: BTreeMap::new(),
        });
        if self.state.transfer.is_some() {
            // A transfer this member ran before it restarted is over.
            self.state.transfer = None;
        }
        // The first entry of every term. Committing it commits everything before it.
        //
        // A resumed `led_term` writes two, sends them only once they are durable, and commits nothing below the
        // second. Its AppendEntries left before its own writes were durable, so the reset may have lost an entry at
        // `next` that a follower holds, in this term. Its own new entry at `next` then differs from that ghost at the
        // same (term, index), which a vote compares equal. Committing only at `next + 1` or above means every member
        // of the committing quorum holds (term, next + 1), so it refuses a ghost holder's vote as behind. Sending them
        // only once durable keeps a second reset from leaving ghosts at both indexes. Otherwise at most the one entry
        // in flight is not yet durable, so there is at most one ghost, at the stored log's end + 1.
        for _ in 0..if resumed { 2 } else { 1 } {
            let e = LogEntry {
                term: self.state.term,
                index: self.last().next(),
                prev: self.digest_at(self.last()).expect("tail digest"),
                format: crate::RAFT_FORMAT,
                tiers: TierSet::NONE,
                at: self.now,
                requests: SmallVec::new(),
                extend: self.take_extend(),
                body: EntryBody::Noop,
            };
            self.push_entry(e);
        }
        let last = self.last();
        let lead = self.lead.as_mut().expect("just set");
        lead.ready_at = last;
        if resumed {
            lead.commit_floor = last;
            self.early_from = last;
        }
        self.broadcast(true);
        self.after_ack();
    }

    /// Ends this leadership. Every ticket it holds, queued, being decided, or in flight, answers `NotLeader`, and the
    /// caller re-sends with the same request id. An in-flight entry may still commit under the next leader, which
    /// then answers the re-send from its `request` row.
    pub(super) fn end_leadership(&mut self) {
        let Some(lead) = self.lead.take() else { return };
        self.rounds = lead.round;
        self.seqs = lead.seq;
        // A leadership that ends while its `STEP_DOWN` transfer runs ended by STEP_DOWN.
        if lead.transfer.as_ref().is_some_and(|t| t.step_down) {
            self.state.stepped_down_at = Some(self.now);
            self.dirty = true;
        }
        self.role = Role::Follower;
        if self.leader == Some(self.cfg.me) {
            self.leader = None;
        }
        let nl = self.not_leader();
        let seqs = lead
            .admission
            .drain()
            .map(|q| q.seq)
            .chain(lead.deciding.iter().flat_map(|d| d.batch.iter().map(|q| q.seq)))
            .chain(lead.pending.iter().flat_map(|(_, d)| d.iter().map(|(s, _, _)| *s)));
        // Each ticket as it was issued: `step_down` raised the term first, and a host keys its waiters and admitted
        // commands by the issued ticket (LM-1 round 19, `tests/m0/results/admitted-bytes.md`).
        let (term, boot) = (lead.term, self.boot);
        let tickets: Vec<Ticket> = seqs.map(|seq| Ticket { term, boot, seq }).collect();
        for ticket in tickets {
            self.out.resolutions.push(Resolution::Ticket { ticket, result: Err(CommitError::NotLeader(nl.clone())) });
        }
        for (id, _, _) in lead.reads {
            self.read_resolved(id, Err(nl.clone()));
        }
        for block in lead.block_pending.map(|(_, b)| b).into_iter().chain(lead.blocks) {
            self.out.resolutions.push(Resolution::IdBlock { block, result: Err(nl.clone()) });
        }
        for id in lead.record_pending.map(|(_, id)| id).into_iter().chain(lead.records.into_iter().map(|r| r.0)) {
            self.out.resolutions.push(Resolution::Recorded { id, result: Err(nl.clone()) });
        }
    }

    /// Records `events` that no command decided, as their own `EntryBody::Events` entry under request id `id`: what
    /// the group's leader commits about its own state, such as a repository's `Promoted` and `Archived`
    /// (`design/DESIGN.md`, Shape, "Packs"). The host checks the events against the fold first; followers `verify`
    /// them as for any entry. The entry takes its turn as the one entry in flight, after id blocks and ahead of queued
    /// commands, and resolves as `Resolution::Recorded { id }` when it commits or the leadership ends. Its `request`
    /// row makes it idempotent: an id that already has a row resolves at that row's index and appends nothing, so a
    /// host whose entry was lost with its leader records it again under the same id. `format` must be one this
    /// release reads.
    pub fn record(
        &mut self,
        now: Timestamp,
        id: RequestId,
        format: Format,
        events: Vec<crate::Ev<C>>,
    ) -> Ready<C, Result<(), CallError>> {
        self.begin(now);
        let r = self.take_record(id, format, events);
        self.finish(r)
    }

    fn take_record(&mut self, id: RequestId, format: Format, events: Vec<crate::Ev<C>>) -> Result<(), CallError> {
        if !self.cfg.reads.contains(&format) {
            return Err(CallError::Unsupported(crate::Unsupported { release: self.cfg.release.id, format }));
        }
        let len: usize = events.iter().map(crate::fold::Record::encoded_len).sum();
        if len > ENTRY_MAX {
            return Err(CallError::TooLarge { len, max: ENTRY_MAX });
        }
        self.lead_or_resume().map_err(CallError::NotLeader)?;
        if self.no_lead() {
            return Err(CallError::NotLeader(self.not_leader()));
        }
        let lead = self.lead.as_mut().expect("leading");
        if lead.record_pending.is_some_and(|(_, r)| r == id) || lead.records.iter().any(|(r, _, _)| *r == id) {
            return Ok(());
        }
        lead.records.push_back((id, format, events));
        self.pump();
        Ok(())
    }

    /// Records an id block the cell's `Ids` group leased to this group, as its own `EntryBody::IdBlock` entry, before
    /// the group assigns from it. The entry takes its turn as the one entry in flight, ahead of queued commands, and
    /// resolves as `Resolution::IdBlock` when it commits or the leadership ends. A block that does not start at or
    /// after the end of the group's latest one, committed, in flight, or queued, is refused at once: re-recording a
    /// block that already committed answers `Behind`, and the host reads the fold for the ids it holds.
    pub fn id_block(&mut self, now: Timestamp, block: IdBlock) -> Ready<C, Result<(), IdBlockError>> {
        self.begin(now);
        let r = self.take_id_block(block);
        self.finish(r)
    }

    /// Appends `next` as the group's config, one change at a time (`design/DESIGN.md`, Shape, `GroupConfig`). The
    /// change may add or remove one voter site, and must keep every other site's member, so a member retried or
    /// replaced on its own site goes out through `Degraded { lost }` and comes back as a new member in a second
    /// change: votes and acks are counted by site, so one site cannot name two members of one config. The previous
    /// config must have committed, the leader must be ready with nothing in flight, and it must stay a voter;
    /// transfer first to replace the leader's own site. The config counts from the moment it is appended; the host
    /// reads `committed_config` for the commit.
    pub fn reconfigure(&mut self, now: Timestamp, next: GroupConfig) -> Ready<C, Result<Index, ReconfigError>> {
        self.begin(now);
        let r = self.take_config(next);
        self.finish(r)
    }

    fn take_config(&mut self, next: GroupConfig) -> Result<Index, ReconfigError> {
        let Some(lead) = &self.lead else { return Err(ReconfigError::NotLeader(self.not_leader())) };
        if !lead.ready || lead.pending.is_some() || !lead.deciding.is_empty() || lead.transfer.is_some() {
            return Err(ReconfigError::Busy);
        }
        if self.configs.last().is_some_and(|i| *i > self.state.commit) {
            return Err(ReconfigError::Busy);
        }
        let cur = self.log_config();
        let names = |c: &GroupConfig| -> BTreeMap<SiteId, Box<str>> {
            let ps: Vec<&graftyard_types::Placement> = match c {
                GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. } => vec![p],
                GroupConfig::Joint { old, new } => vec![old, new],
            };
            let voters = quorum::voters(c);
            ps.iter()
                .flat_map(|p| p.voters().iter())
                .filter(|m| voters.contains(&m.site))
                .map(|m| (m.site, m.do_name.clone()))
                .collect()
        };
        // Each member's `do_name` by address (`Member::addr`): every voter, and every learner, by its slot's address.
        let members = |c: &GroupConfig| -> BTreeMap<SiteId, Box<str>> {
            let voters = quorum::voters(c);
            let ps: Vec<&graftyard_types::Placement> = match c {
                GroupConfig::Normal(p) | GroupConfig::Degraded { placement: p, .. } => vec![p],
                GroupConfig::Joint { old, new } => vec![old, new],
            };
            ps.iter()
                .flat_map(|p| p.voters().iter().filter(|m| voters.contains(&m.site)).chain(p.learners()))
                .map(|m| (m.addr(), m.do_name.clone()))
                .collect()
        };
        let (a, b) = (names(cur), names(&next));
        // Every address, voter or learner, whose member is new: absent before, or named differently.
        let (was, now) = (members(cur), members(&next));
        let added: Vec<SiteId> = now.iter().filter(|(s, n)| was.get(*s) != Some(*n)).map(|(s, _)| *s).collect();
        let changed = a.keys().filter(|s| !b.contains_key(s)).count() + b.keys().filter(|s| !a.contains_key(s)).count();
        if changed > 1 || a.iter().any(|(s, n)| b.get(s).is_some_and(|m| m != n)) {
            return Err(ReconfigError::NotOneChange);
        }
        if !b.contains_key(&self.cfg.me) {
            return Err(ReconfigError::RemovesLeader);
        }
        let last = self.last();
        let e = LogEntry {
            term: self.state.term,
            index: last.next(),
            prev: self.digest_at(last).expect("tail digest"),
            format: crate::RAFT_FORMAT,
            tiers: TierSet::NONE,
            at: self.now,
            requests: SmallVec::new(),
            extend: self.take_extend(),
            body: EntryBody::Config(next),
        };
        let index = e.index;
        self.push_entry(e);
        // A new member at an address keeps nothing the old member there answered: its old `matched` would count toward
        // a quorum the new member is not in, and a new, empty learner whose slot kept `matched` was probed at that
        // index forever and never caught up (B0-33, LM-1 round 23: 1,776 calls, 144.9 MB, all refused). It starts
        // from the leader's next index and backs up from its answers.
        let next_index = self.last();
        let lead = self.lead.as_mut().expect("leading");
        for p in added {
            lead.member_from.insert(p, lead.round + 1);
            lead.next.insert(p, next_index);
            lead.matched.insert(p, Index(0));
            lead.matched_at.remove(&p);
            lead.sent.remove(&p);
            lead.pipe.remove(&p);
            lead.promised_at.remove(&p);
            lead.acked_at.remove(&p);
            lead.retries.remove(&p);
            lead.refusals.remove(&p);
            lead.told.remove(&p);
            lead.telling.remove(&p);
        }
        self.lead.as_mut().expect("leading").pending = Some((index, Vec::new()));
        self.broadcast(false);
        self.advance_commit();
        Ok(index)
    }

    fn take_id_block(&mut self, block: IdBlock) -> Result<(), IdBlockError> {
        self.lead_or_resume().map_err(IdBlockError::NotLeader)?;
        if self.no_lead() {
            return Err(IdBlockError::NotLeader(self.not_leader()));
        }
        // The latest block this leader holds: queued, in flight, in its uncommitted log (a restarted leader resumes
        // with the block an earlier leadership appended), or committed. A re-sent lease of a block already in the log
        // was appended twice, both followers refused the copy, and the leader reused that index in its term.
        let applied = self.fold.applied();
        let logged = self.log.iter().rev().take_while(|e| e.index > applied).find_map(|e| match &e.body {
            EntryBody::IdBlock(b) => Some(*b),
            _ => None,
        });
        let committed = logged.or(self.fold.id_block());
        let lead = self.lead.as_mut().expect("leading");
        if lead.block_pending.map(|(_, b)| b) == Some(block) || lead.blocks.contains(&block) {
            return Ok(());
        }
        let latest = lead.blocks.back().copied().or(lead.block_pending.map(|(_, b)| b)).or(committed);
        if let Some(current) = latest
            && !current.followed_by(block)
        {
            return Err(IdBlockError::Behind { current });
        }
        lead.blocks.push_back(block);
        self.pump();
        Ok(())
    }

    /// Takes a command into the next entry as Human, with no actor: it is never demoted, staged, or refused by
    /// admission, and Human commands keep their order. A restarted leader resumes `led_term` here.
    pub fn commit(&mut self, now: Timestamp, id: RequestId, cmd: C) -> Ready<C, Result<Admitted, CallError>> {
        self.begin(now);
        let r = self.admit(id, cmd, Admit::human()).map_err(|e| match e {
            AdmitError::Call(e) => e,
            AdmitError::Full { .. } | AdmitError::Stage => unreachable!("a Human command with no pack is admitted"),
        });
        self.finish(r)
    }

    /// `commit` under admission (`crate::admit`): the edge's class, the actor, and the command's inline pack bytes.
    /// The leader demotes a Human or Bot actor past DEMOTE_PER_MIN committed and queued writes in the last minute to
    /// Agent. An Agent command is refused with `Full` at AGENT_QUEUE_MAX, and so is an agent pack over INLINE_MAX_HOT
    /// or past AGENT_INLINE_ENTRY while placement fails. `Stage` asks the edge to store the pack and send again: a
    /// pack over INLINE_MAX_HOT on a hot group, an agent pack under pressure, and one past AGENT_INLINE_ENTRY. A
    /// request id already queued or committed joins its ticket or row whatever it carries.
    pub fn commit_as(
        &mut self,
        now: Timestamp,
        id: RequestId,
        cmd: C,
        a: Admit,
    ) -> Ready<C, Result<Admitted, AdmitError>> {
        self.begin(now);
        let r = self.admit(id, cmd, a);
        self.finish(r)
    }

    /// The host's fixed admission capacity in commands a second, or `None` to measure it (the default). The M0-WP9
    /// prototype's `capN` groups did this to make the agent bucket bind at a known rate.
    pub fn set_admit_capacity(&mut self, cap: Option<f32>) {
        self.admit_capacity = cap;
    }

    /// The host's byte limit for filling an entry past its first command, ENTRY_SOFT by default. Test installs set
    /// ENTRY_MAX to run B0-02's uncapped arm in the same window (`tests/m0/results/carried-7.md`).
    pub fn set_entry_soft(&mut self, bytes: usize) {
        self.entry_soft = bytes.min(ENTRY_MAX);
    }

    /// Whether the host's clock may read DRIFT / 2 or more behind real time (`design/DESIGN.md`, Shape, `DRIFT`): a
    /// busy isolate's clock can read up to seconds before the event it handles, so a lease check on it could serve past
    /// the lease. While set when a read arrives, `fresh` grants it only after a round sent after its arrival, which
    /// rests on the quorum's acks, not on this clock. The flag is read at arrival only: a read that arrived on a live
    /// lease and a trusted clock is linearized at its arrival, so a later grant on that lease is still safe.
    pub fn set_clock_suspect(&mut self, suspect: bool) {
        self.clock_suspect = suspect;
    }

    /// The isolate's own clock at this input, before any floor (`design/DESIGN.md`, "The read path"). A Workers
    /// isolate's clock moves only between I/O turns, so inputs in one turn share it, while the member clock, floored
    /// at each newer peer stamp, can jump many times in that turn. `read_round` times its gap on this clock and sends
    /// at most one round a reading. Unset, as in the simulator, it is the member clock.
    /// Never below a reading it gave before, so a clock stepped back reads as one that did not move.
    pub fn set_own_clock(&mut self, own: Timestamp) {
        self.own_clock = Some(self.own_clock.map_or(own, |o| o.max(own)));
    }

    /// Whether this leader is quiet enough to hand its group back to `home` (`maybe_hand_off`, `home-quiet`): at most
    /// `queue` commands wait in admission's lanes (`AdmissionView::queued`), and the entry in flight, if any, holds at
    /// most half of `entry_soft`. A transfer stalls whatever waits on the leader until the new one serves.
    pub(super) fn quiet(&self, queue: usize) -> bool {
        let Some(lead) = &self.lead else { return false };
        let queued: usize = lead.admission.lanes.iter().map(Lane::len).sum();
        let entry = lead.pending.as_ref().and_then(|(i, _)| self.entry_len(*i)).unwrap_or(0);
        queued <= queue && entry <= self.entry_soft / 2
    }

    /// What admission sees now, as leader: capacity, queued agent commands and bytes, whether the group is hot and
    /// under pressure. `None` on a follower.
    pub fn admission(&self) -> Option<AdmissionView> {
        let lead = self.lead.as_ref()?;
        let ad = &lead.admission;
        let cap = ad.capacity(self.admit_capacity, self.entry_soft);
        let rate = ad.agent_rate(self.now, cap);
        Some(AdmissionView {
            capacity: cap,
            agent_rate: rate,
            agent_wait: expected_wait(ad.agent().len(), rate, ad.round_ms),
            agent_queued: ad.agent().len(),
            agent_bytes: ad.agent_bytes,
            queued: ad.lanes.iter().map(Lane::len).sum(),
            hot: self.hot(),
            pressure: self.pressure(cap),
            retry_after_s: retry_after_s(ad.agent().len(), rate),
            round_ms: ad.round_ms,
            cmd_bytes: ad.cmd_bytes,
            held: lead.deciding.len(),
        })
    }

    /// The bytes of the commands this leader holds for tickets it has not resolved: each queued or frozen command's
    /// encoded length, and for the entry in flight what each decided outcome keeps that no event stores (its reply
    /// render). For the host's memory split (`inFlight.admittedBytes`).
    pub fn admitted_bytes(&self) -> usize {
        let Some(lead) = &self.lead else { return 0 };
        let queued: usize = lead.admission.lanes.iter().flat_map(Lane::iter).map(|q| q.bytes).sum();
        let frozen: usize = lead.deciding.iter().flat_map(|d| &d.batch).map(|q| q.bytes).sum();
        let decided: usize = lead.pending.iter().flat_map(|(_, d)| d).map(|(_, unstored, _)| unstored).sum();
        queued + frozen + decided
    }

    /// Pushes a second over the last HOT_WINDOW of committed entries, above HOT_PUSHES.
    pub(super) fn hot(&self) -> bool {
        let w = HOT_WINDOW.as_millis() as u64;
        let n = self.fold.pushes(Timestamp(self.now.0.saturating_sub(w)), Timestamp(self.now.0 + 1));
        n as f32 / HOT_WINDOW.as_secs_f32() > HOT_PUSHES
    }

    fn pressure(&self, cap: f32) -> bool {
        let Some(lead) = &self.lead else { return false };
        let ad = &lead.admission;
        ad.agent_bytes > ENTRY_MAX || ad.agent().len() as f32 > AGENT_SHARE * cap * PRESSURE_AGE.as_secs_f32()
    }

    /// DEMOTE_PER_MIN: the actor's committed writes in the last minute, plus those this leader holds unresolved.
    fn demoted(&self, a: &Admit) -> AdmitClass {
        let (Some(actor), AdmitClass::Human | AdmitClass::Bot) = (a.actor, a.class) else { return a.class };
        let from = Timestamp(self.now.0.saturating_sub(DEMOTE_WINDOW.as_millis() as u64));
        let committed = self.fold.actor_writes(actor, from, Timestamp(self.now.0 + 1));
        let ad = self.lead.as_ref().map(|l| &l.admission);
        let held = ad.and_then(|ad| ad.pending_by.get(&actor)).copied().unwrap_or(0);
        let refused = ad.map_or(0, |ad| ad.refused_since(actor, from));
        if committed + held + refused >= DEMOTE_PER_MIN { AdmitClass::Agent } else { a.class }
    }

    /// The admission rules for a new request id, in the design's order: the outage refusal, AGENT_QUEUE_MAX before
    /// any staging, then staging. Returns the class it queues under.
    fn admission_check(&self, a: &Admit, bytes: usize) -> Result<AdmitClass, AdmitError> {
        let lead = self.lead.as_ref().expect("leading");
        let ad = &lead.admission;
        let class = self.demoted(a);
        let agent = class == AdmitClass::Agent;
        let cap = ad.capacity(self.admit_capacity, self.entry_soft);
        let rate = ad.agent_rate(self.now, cap);
        let full = || AdmitError::Full { retry_after_s: retry_after_s(ad.agent().len(), rate) };
        if agent && !a.placement_ok && a.inline > INLINE_MAX_HOT {
            return Err(full());
        }
        // AGENT_PUSH_BOUND: an agent command whose queue ahead, at the agent rate, plus one round is longer than what
        // is left of its bound is refused now, not queued to be refused later.
        if agent && a.budget.is_some_and(|b| expected_wait(ad.agent().len(), rate, ad.round_ms) > b) {
            return Err(full());
        }
        let hot_stage = a.push && a.placement_ok && a.inline > INLINE_MAX_HOT && self.hot();
        let agent_stage =
            agent && a.inline > 0 && (self.pressure(cap) || ad.agent_inline + a.inline > AGENT_INLINE_ENTRY);
        let staged = if hot_stage || agent_stage { bytes - a.inline } else { bytes };
        if agent && (ad.agent().len() >= AGENT_QUEUE_MAX.0 as usize || ad.agent_bytes + staged > AGENT_QUEUE_MAX.1) {
            return Err(full());
        }
        if agent_stage && !a.placement_ok {
            return Err(full());
        }
        if hot_stage || agent_stage {
            return Err(AdmitError::Stage);
        }
        Ok(class)
    }

    fn admit(&mut self, id: RequestId, cmd: C, a: Admit) -> Result<Admitted, AdmitError> {
        let call = AdmitError::Call;
        let len = cmd.encoded_len();
        if len > COMMAND_MAX {
            return Err(call(CallError::TooLarge { len, max: COMMAND_MAX }));
        }
        let format = cmd.format();
        if !self.cfg.reads.contains(&format) {
            return Err(call(CallError::Unsupported(crate::Unsupported { release: self.cfg.release.id, format })));
        }
        self.lead_or_resume().map_err(|nl| call(CallError::NotLeader(nl)))?;
        if self.no_lead() {
            return Err(call(CallError::NotLeader(self.not_leader())));
        }
        self.expire_agents();
        let joins = self.lead.as_ref().expect("leading").by_request.get(&id).copied();
        let mut admitted = a.class;
        let mut demoted = None;
        let seq = match joins {
            Some(seq) => seq,
            None => {
                // A command that already committed is answered from its row in the next entry cut, whatever class.
                let class = if self.fold.request(id).is_some() {
                    a.class
                } else {
                    let r = self.admission_check(&a, len);
                    if matches!(r, Err(AdmitError::Full { .. })) {
                        let now = self.now;
                        self.lead.as_mut().expect("leading").admission.refused(a.actor, now);
                    }
                    r?
                };
                admitted = class;
                if class != a.class {
                    demoted = a.actor;
                }
                let now = self.now;
                let lead = self.lead.as_mut().expect("leading");
                lead.seq += 1;
                let seq = lead.seq;
                lead.by_request.insert(id, seq);
                let ad = &mut lead.admission;
                let agent_inline = if class == AdmitClass::Agent { a.inline } else { 0 };
                let soft_bytes = cmd.stored_len().saturating_sub(agent_inline);
                ad.cmd_bytes = 0.98 * ad.cmd_bytes + 0.02 * soft_bytes as f32;
                if class == AdmitClass::Agent {
                    ad.agent_bytes += len;
                    ad.agent_inline += agent_inline;
                }
                if let Some(actor) = a.actor {
                    *ad.pending_by.entry(actor).or_insert(0) += 1;
                    ad.seq_actor.insert(seq, actor);
                }
                let wait = match a.budget {
                    Some(b) if class == AdmitClass::Agent => b.min(AGENT_WAIT),
                    _ => AGENT_WAIT,
                };
                let until = Timestamp(now.0 + wait.as_millis() as u64);
                let q = Queued { seq, id, cmd, bytes: len, agent_inline, soft_bytes, until };
                ad.lanes[class.lane()].push(a.actor, q);
                seq
            }
        };
        let ticket = self.ticket(seq);
        self.pump();
        Ok(Admitted { ticket, class: admitted, demoted })
    }

    fn lead_or_resume(&mut self) -> Result<(), NotLeader> {
        if self.role == Role::Leader {
            return Ok(());
        }
        if self.state.led_term == Some(self.state.term)
            && self.campaign.is_none()
            && self.fold.config().is_some()
            && !self.no_lead()
        {
            self.become_leader(true);
            return Ok(());
        }
        Err(self.not_leader())
    }

    /// The ticket's result if it has resolved, `None` while it is pending. A ticket from another term or `boot`
    /// answers `NotLeader`.
    pub fn outcome(&self, ticket: Ticket) -> Option<Outcome<C>> {
        let nl = || Some(Err(CommitError::NotLeader(self.not_leader())));
        let Some(lead) = &self.lead else { return nl() };
        if ticket.term != self.state.term || ticket.boot != self.boot {
            return nl();
        }
        if let Some((_, _, r)) = lead.done.get(&ticket.seq) {
            return Some(r.clone());
        }
        let pending = lead.by_request.values().any(|s| *s == ticket.seq);
        if pending { None } else { nl() }
    }

    /// An edge whose `commit` timed out asks by request id. The design passes only the id; replaying the stored
    /// events needs the command, so the caller passes the command it would re-send.
    pub fn outcome_by_request(&self, id: RequestId, cmd: &C) -> Result<ByRequest<C>, NotLeader> {
        let lead = match &self.lead {
            Some(l) if l.ready => l,
            _ => return Err(self.not_leader()),
        };
        if let Some(row) = self.fold.request(id) {
            let proof = lead.proof.as_ref().expect("ready implies a commit proof");
            return Ok(ByRequest::Done(outcome::replay(row.index, proof, cmd.replay(&row.events))));
        }
        Ok(match lead.by_request.get(&id) {
            Some(seq) => ByRequest::Pending(self.ticket(*seq)),
            None => ByRequest::Unknown,
        })
    }

    /// One entry in flight. Once the previous entry committed, the leader appends the next id block or recorded
    /// entry, else decides the held batch, else cuts a batch and reads its dependencies. While an entry is in flight
    /// it may cut the next batch early (`cut_early`), so that batch's reads overlap the round; the batch is decided
    /// only here, after the entry in flight committed and applied.
    fn pump(&mut self) {
        loop {
            let no_lead = self.no_lead();
            let Some(lead) = self.lead.as_mut() else { return };
            // A transferring leader stops appending, and one with `no_lead_until` is handing off.
            if !lead.ready || lead.transfer.is_some() || no_lead {
                return;
            }
            if let Some((index, _)) = lead.pending {
                self.cut_early(index);
                return;
            }
            if let Some(block) = lead.blocks.pop_front() {
                self.append_id_block(block);
                return;
            }
            if let Some((id, format, events)) = lead.records.pop_front() {
                // Recorded already, under an earlier leadership: answer from its row.
                if let Some(row) = self.fold.request(id) {
                    let index = row.index;
                    self.out.resolutions.push(Resolution::Recorded { id, result: Ok(index) });
                    continue;
                }
                self.append_record(id, format, events);
                return;
            }
            if !lead.deciding.is_empty() {
                if self.decide_held() {
                    continue;
                }
                return;
            }
            if lead.admission.is_empty() {
                return;
            }
            let Some(fresh) = self.cut_fresh() else { return };
            if fresh.is_empty() {
                continue;
            }
            let chains = self.chains(&fresh);
            if chains.is_empty() {
                self.decide(fresh, Vec::new(), SmallVec::new());
            } else {
                self.hold_batch(fresh);
                self.ask_deps(chains);
            }
            return;
        }
    }

    /// While entry `index` is in flight: cut a later batch and ask for its dependencies, so its read ends about when
    /// the entry before it commits. With `held` batches already frozen, the next is entry `index + held + 1`'s, due
    /// `held + 1` expected rounds after `index` was appended less one expected read; at most `read_ahead` are held.
    /// Not before this leadership measured a read, nor while an id block or recorded entry waits, since those go next.
    /// A batch holds the commands admitted before its read was asked for; later ones wait for a later entry.
    fn cut_early(&mut self, index: Index) {
        let Some(lead) = self.lead.as_ref() else { return };
        if lead.admission.is_empty() || !lead.blocks.is_empty() || !lead.records.is_empty() {
            return;
        }
        let Some(deps_ms) = lead.admission.deps_ms else { return };
        // `reask-read`: the newest batch, cut with nothing left queued, takes what arrives behind it instead of a new
        // batch being cut, asking a read of every chain it and the queue name, one read out at a time.
        let one_ahead = read_ahead(lead.admission.round_ms, deps_ms) == 1;
        if let Some(d) = lead.deciding.back()
            && d.open
            && one_ahead
            && d.behind == index
        {
            if d.id.is_none() && d.reask.is_none() && lead.transfer.is_none() && !lead.admission.is_empty() {
                let fold = &self.fold.fold;
                let mut chains: SmallVec<[ChainId; 3]> = SmallVec::new();
                let queued = lead.admission.lanes.iter().flat_map(Lane::iter);
                for c in d.batch.iter().chain(queued).flat_map(|q| q.cmd.deps(fold)) {
                    if !chains.contains(&c) {
                        chains.push(c);
                    }
                }
                self.next_deps = self.next_deps.wrapping_add(1);
                let (id, now) = (self.next_deps, self.now);
                let lead = self.lead.as_mut().expect("leading");
                let upto = lead.seq;
                lead.deciding.back_mut().expect("a held batch").reask = Some((id, upto, chains.clone(), now));
                self.out.deps.push(DepsWanted { id, chains });
            }
            return;
        }
        let round_ms = lead.admission.round_ms;
        let held = lead.deciding.len();
        if held >= read_ahead(round_ms, deps_ms) {
            return;
        }
        let Some(at) = self.entry_at(index).map(|e| e.at) else { return };
        let wait = ((held + 1) as f32 * round_ms - deps_ms).max(0.0) as u64;
        if self.now.0 < at.0.saturating_add(wait) {
            return;
        }
        // A batch with no dependency gains nothing from an early cut, and would keep out the commands that arrive
        // before the entry commits.
        let fold = &self.fold.fold;
        if !lead.admission.lanes.iter().flat_map(Lane::iter).any(|q| !q.cmd.deps(fold).is_empty()) {
            return;
        }
        let Some(fresh) = self.cut_fresh() else { return };
        if fresh.is_empty() {
            return;
        }
        let note = *self.out.cuts.last().expect("cut_fresh noted its cut");
        let chains = self.chains(&fresh);
        self.hold_batch(fresh);
        let lead = self.lead.as_mut().expect("leading");
        // Only the newest batch is open; the one cut before it closes.
        let n = lead.deciding.len();
        if n >= 2 {
            lead.deciding[n - 2].open = false;
            lead.deciding[n - 2].reask = None;
        }
        let d = lead.deciding.back_mut().expect("a held batch");
        // Reads that outlast a round (`read_ahead` over 1) freeze a batch per round instead, as before.
        d.open = note.end == CutEnd::Empty && note.in_flight && !chains.is_empty() && n == 1 && one_ahead;
        d.behind = index;
        d.agents = note.taken[AdmitClass::Agent.lane()];
        if !chains.is_empty() {
            self.ask_deps(chains);
        }
    }

    /// The next batch by admission (`cut`), less the commands whose request already has a row, which are answered
    /// from their stored events. `None` when admission gave nothing: only agent commands wait, and the bucket is empty,
    /// so `tick` cuts again once it refills.
    fn cut_fresh(&mut self) -> Option<Vec<Queued<C>>> {
        let lead = self.lead.as_mut().expect("leading");
        let (batch, note) = Self::cut(lead, self.now, self.admit_capacity, self.entry_soft);
        if batch.is_empty() {
            return None;
        }
        self.out.cuts.push(note);
        Some(self.drop_replays(batch))
    }

    /// A command whose id has a row is not decided again: answer it from the stored events.
    fn drop_replays(&mut self, batch: Vec<Queued<C>>) -> Vec<Queued<C>> {
        let proof = self.lead.as_ref().expect("leading").proof.clone().expect("ready implies a commit proof");
        let mut fresh = Vec::with_capacity(batch.len());
        for q in batch {
            match self.fold.request(q.id) {
                Some(row) => {
                    let acked = outcome::replay(row.index, &proof, q.cmd.replay(&row.events));
                    self.resolve(q.seq, Some(q.id), q.unstored(), Ok(acked));
                }
                None => fresh.push(q),
            }
        }
        fresh
    }

    /// The distinct chains the batch's commands name on the fold at `applied`, in order.
    fn chains(&self, batch: &[Queued<C>]) -> SmallVec<[ChainId; 3]> {
        let mut chains: SmallVec<[ChainId; 3]> = SmallVec::new();
        for q in batch {
            for c in q.cmd.deps(&self.fold.fold) {
                if !chains.contains(&c) {
                    chains.push(c);
                }
            }
        }
        chains
    }

    fn hold_batch(&mut self, batch: Vec<Queued<C>>) {
        let now = self.now;
        let lead = self.lead.as_mut().expect("leading");
        lead.deciding.push_back(Deciding {
            id: None,
            batch,
            chains: SmallVec::new(),
            deps: Vec::new(),
            asked: now,
            ids: SmallVec::new(),
            open: false,
            behind: Index(0),
            agents: 0,
            reask: None,
        });
    }

    /// Asks the host to read `chains` for the batch cut last, under an id no earlier ask of this member used.
    fn ask_deps(&mut self, chains: SmallVec<[ChainId; 3]>) {
        self.ask_deps_for(false, chains);
    }

    /// `ask_deps` for the front batch (`front`), the next to decide, or the one cut last.
    fn ask_deps_for(&mut self, front: bool, chains: SmallVec<[ChainId; 3]>) {
        self.next_deps = self.next_deps.wrapping_add(1);
        let id = self.next_deps;
        let now = self.now;
        let held = &mut self.lead.as_mut().expect("leading").deciding;
        let d = if front { held.front_mut() } else { held.back_mut() }.expect("a held batch");
        d.id = Some(id);
        d.asked = now;
        d.chains.extend(chains.iter().copied());
        self.out.deps.push(DepsWanted { id, chains });
    }

    /// The entry in flight committed and applied. Decides the held batch on the fold now, with exactly the chains
    /// `deps()` names on it, in that order: a chain no read covered yet is read first, and a read chain no longer
    /// named is dropped, so `decide` never sees the dependency of a chain the fold has moved away from. Returns
    /// whether the batch is gone with nothing decided, so the next one may be cut.
    fn decide_held(&mut self) -> bool {
        let lead = self.lead.as_mut().expect("leading");
        let d = lead.deciding.front_mut().expect("a held batch");
        // `reask-read`: the entry before it applied, so the batch takes no more, and an unanswered re-ask is dropped:
        // the batch decides on the newest read that answered, which every command in it was admitted before.
        d.open = false;
        d.reask = None;
        if d.id.is_some() {
            return false;
        }
        let batch = std::mem::take(&mut d.batch);
        let batch = self.drop_replays(batch);
        if batch.is_empty() {
            self.lead.as_mut().expect("leading").deciding.pop_front();
            return true;
        }
        let chains = self.chains(&batch);
        let d = self.lead.as_mut().and_then(|l| l.deciding.front_mut()).expect("a held batch");
        d.batch = batch;
        let missing: SmallVec<[ChainId; 3]> = chains.iter().copied().filter(|c| !d.chains.contains(c)).collect();
        if !missing.is_empty() {
            self.ask_deps_for(true, missing);
            return false;
        }
        let Deciding { batch, deps: mut have, ids, .. } =
            self.lead.as_mut().expect("leading").deciding.pop_front().expect("a held batch");
        let deps =
            chains.iter().filter_map(|c| have.iter().position(|(k, _)| k == c).map(|i| have.swap_remove(i))).collect();
        self.decide(batch, deps, ids);
        false
    }

    /// Ends every held batch's reads: a transfer may outlast them by a LEASE, and a revocation acknowledged meanwhile
    /// must bind each batch, so once the transfer ends each batch reads every chain again.
    pub(super) fn drop_held_deps(&mut self) {
        for d in self.lead.as_mut().map(|l| l.deciding.iter_mut()).into_iter().flatten() {
            d.id = None;
            d.chains.clear();
            d.deps.clear();
            d.ids.clear();
            d.open = false;
            d.reask = None;
        }
    }

    /// The next entry's commands, by admission: Human and MergeQueue, then Agent up to AGENT_FLOOR of capacity over
    /// the time since the last cut, then Bot, then Agent while the AGENT_SHARE bucket holds a token, at most
    /// AGENT_CMDS_ENTRY Agent commands in all. All within `soft`
    /// bytes (ENTRY_SOFT unless the host set it, ENTRY_SOFT_DEEP while the queue is deep) not counting Agent inline
    /// packs, and within ENTRY_MAX counting them;
    /// the first command of an entry always goes, up to ENTRY_MAX. Agent packs have their own bound: admission stages
    /// any that would take the queued ones past AGENT_INLINE_ENTRY. Counted against `soft` as well, one 10 to 70 KB
    /// agent pack filled an entry, and B0-29's commit p99 at 500 agents went from 702 to 1,718 ms (LM-1,
    /// `tests/am1/results/lm1-memory.md`, round 9). Human, MergeQueue and Bot packs, which no other rule bounds, count.
    /// With the batch, what it took and what ended it (`CutNote`).
    fn cut(lead: &mut Lead<C>, now: Timestamp, fixed: Option<f32>, soft: usize) -> (Vec<Queued<C>>, CutNote) {
        let (held, in_flight) = (lead.deciding.len(), lead.pending.is_some());
        let ad = &mut lead.admission;
        let cap = ad.capacity(fixed, soft);
        // soft-grow: with more than DEEP_QUEUE entries' worth of Human and MergeQueue bytes waiting, this entry fills
        // to ENTRY_SOFT_DEEP; the next cut that finds the queue shallow is back at `soft`. Capacity, and so the agent
        // bucket, stays sized from `soft`.
        let first_lanes = [AdmitClass::Human, AdmitClass::MergeQueue];
        let queued: usize = first_lanes.iter().flat_map(|c| ad.lanes[c.lane()].iter()).map(|q| q.soft_bytes).sum();
        let soft = if queued > DEEP_QUEUE * soft { soft.max(ENTRY_SOFT_DEEP) } else { soft };
        ad.refill(now, cap);
        let mut batch: Vec<Queued<C>> = Vec::new();
        let mut taken = [0u32; 4];
        // (bytes ENTRY_SOFT counts, all bytes)
        let mut bytes = (0, 0);
        let fits = |q: Option<&Queued<C>>, batch: &[Queued<C>], b: (usize, usize)| {
            q.is_some_and(|q| batch.is_empty() || (b.0 + q.soft_bytes <= soft && b.1 + q.bytes <= ENTRY_MAX))
        };
        let add = |b: (usize, usize), q: &Queued<C>| (b.0 + q.soft_bytes, b.1 + q.bytes);
        for class in first_lanes {
            while fits(ad.lanes[class.lane()].front(), &batch, bytes) {
                let q = ad.take(class).expect("front");
                (bytes, taken[class.lane()]) = (add(bytes, &q), taken[class.lane()] + 1);
                batch.push(q);
            }
        }
        let since = (now.0.saturating_sub(ad.cut_at.0) as f32).max(ad.round_ms);
        let floor = (AGENT_FLOOR * cap * since / 1000.0).floor() as usize;
        let mut n = 0;
        while n < floor.min(AGENT_CMDS_ENTRY) && ad.tokens >= 1.0 && fits(ad.agent().front(), &batch, bytes) {
            let q = ad.take(AdmitClass::Agent).expect("front");
            (bytes, n, ad.tokens) = (add(bytes, &q), n + 1, ad.tokens - 1.0);
            taken[AdmitClass::Agent.lane()] += 1;
            batch.push(q);
        }
        while fits(ad.lanes[AdmitClass::Bot.lane()].front(), &batch, bytes) {
            let q = ad.take(AdmitClass::Bot).expect("front");
            (bytes, taken[AdmitClass::Bot.lane()]) = (add(bytes, &q), taken[AdmitClass::Bot.lane()] + 1);
            batch.push(q);
        }
        while taken[AdmitClass::Agent.lane()] < AGENT_CMDS_ENTRY as u32
            && ad.tokens >= 1.0
            && fits(ad.agent().front(), &batch, bytes)
        {
            let q = ad.take(AdmitClass::Agent).expect("front");
            (bytes, ad.tokens) = (add(bytes, &q), ad.tokens - 1.0);
            taken[AdmitClass::Agent.lane()] += 1;
            batch.push(q);
        }
        if !batch.is_empty() {
            ad.cut_at = now;
            ad.drain.cut(now, taken[AdmitClass::Agent.lane()], ad.agent().len());
        }
        // A Human, MergeQueue or Bot command left had no lane rule but bytes to stop it; an Agent one, bytes or tokens.
        let past = |q: &Queued<C>| if bytes.0 + q.soft_bytes > soft { CutEnd::Soft } else { CutEnd::EntryMax };
        let first = [AdmitClass::Human, AdmitClass::MergeQueue, AdmitClass::Bot].map(|c| ad.lanes[c.lane()].front());
        let end = match (first.into_iter().flatten().next(), ad.agent().front()) {
            (Some(q), _) => past(q),
            (None, Some(_)) if ad.tokens < 1.0 => CutEnd::AgentTokens,
            (None, Some(_)) if taken[AdmitClass::Agent.lane()] >= AGENT_CMDS_ENTRY as u32 => CutEnd::AgentCmds,
            (None, Some(q)) => past(q),
            (None, None) => CutEnd::Empty,
        };
        let left = AdmitClass::ALL.map(|c| ad.lanes[c.lane()].len() as u32);
        let note =
            CutNote { taken, left, soft_bytes: bytes.0, bytes: bytes.1, tokens: ad.tokens, held, in_flight, end };
        (batch, note)
    }

    /// AGENT_WAIT, or an agent command's own `Admit::budget`: one still queued past its `until` is answered `Full` and
    /// never enters an entry. Only the lanes are searched, so a command already cut into a batch or an entry is never
    /// answered `Full`; it waits for its outcome.
    fn expire_agents(&mut self) {
        let Some(lead) = self.lead.as_mut() else { return };
        let now = self.now;
        let ad = &mut lead.admission;
        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.until < now);
        if old.is_empty() {
            return;
        }
        for q in &old {
            ad.agent_bytes -= q.bytes;
            ad.agent_inline -= q.agent_inline;
        }
        let cap = ad.capacity(self.admit_capacity, self.entry_soft);
        let retry_after_s = retry_after_s(ad.agent().len(), ad.agent_rate(now, cap));
        for q in &old {
            if let Some(a) = ad.seq_actor.get(&q.seq).copied() {
                ad.refused(Some(a), now);
            }
        }
        for q in old {
            self.resolve(q.seq, Some(q.id), 0, Err(CommitError::Full { retry_after_s }));
        }
    }

    /// The dependency folds a `DepsWanted` asked for, each read once through its chain's `fresh`. The held batch that
    /// asked keeps them until the entry before it commits; an answer to any other id, such as one asked for by an earlier
    /// leadership or before a transfer, is ignored.
    pub fn deps_read(&mut self, now: Timestamp, id: u64, deps: Vec<(ChainId, C::Dep)>) -> Ready<C, ()> {
        self.begin(now);
        let now = self.now;
        if let Some(lead) = self.lead.as_mut()
            && let Some(d) = lead.deciding.iter_mut().find(|d| d.id == Some(id))
        {
            d.id = None;
            d.ids.push(id);
            d.deps.extend(deps);
            let ms = now.0.saturating_sub(d.asked.0) as f32;
            let ad = &mut lead.admission;
            ad.deps_ms = Some(ad.deps_ms.map_or(ms, |m| 0.9 * m + 0.1 * ms));
            self.pump();
        } else if let Some(lead) = self.lead.as_mut()
            && let Some(at) = lead.deciding.iter().position(|d| d.reask.as_ref().is_some_and(|r| r.0 == id))
        {
            // `reask-read`: a re-ask that read every chain it asked replaces the batch's reads, so every command the
            // entry decides sees this one read, asked after each was admitted; then the commands admitted before it
            // was asked join the batch. A re-ask missing a chain changes nothing, and the next one is asked.
            let d = &mut lead.deciding[at];
            let (_, upto, chains, asked) = d.reask.take().expect("matched");
            if chains.iter().all(|c| deps.iter().any(|(k, _)| k == c)) {
                d.deps = deps;
                d.ids = SmallVec::from_slice(&[id]);
                d.chains = chains;
                let ms = now.0.saturating_sub(asked.0) as f32;
                lead.admission.deps_ms = Some(lead.admission.deps_ms.map_or(ms, |m| 0.9 * m + 0.1 * ms));
                let behind = lead.deciding[at].behind;
                if at + 1 == lead.deciding.len() && lead.pending.as_ref().is_some_and(|(i, _)| *i == behind) {
                    self.top_up(upto);
                }
            }
            self.pump();
        }
        self.finish(())
    }

    /// `reask-read`: moves the commands admitted at or before seq `upto` and still queued into the newest batch, in
    /// admission order, within the batch's caps (`entry_soft` and ENTRY_MAX in bytes, AGENT_CMDS_ENTRY and the agent
    /// bucket for Agent commands). The first that does not fit closes the batch and stays queued with every later one.
    fn top_up(&mut self, upto: u64) {
        let (now, soft, fixed) = (self.now, self.entry_soft, self.admit_capacity);
        let lead = self.lead.as_mut().expect("leading");
        let d = lead.deciding.back_mut().expect("a held batch");
        let ad = &mut lead.admission;
        let cap = ad.capacity(fixed, soft);
        ad.refill(now, cap);
        let mut queued: Vec<(u64, AdmitClass, usize, usize)> = AdmitClass::ALL
            .iter()
            .flat_map(|c| ad.lanes[c.lane()].iter().map(|q| (q.seq, *c, q.soft_bytes, q.bytes)))
            .filter(|q| q.0 <= upto)
            .collect();
        queued.sort_unstable_by_key(|q| q.0);
        let (mut s, mut b) = d.batch.iter().fold((0, 0), |(s, b), q| (s + q.soft_bytes, b + q.bytes));
        let mut take = BTreeSet::new();
        for (seq, class, qs, qb) in queued {
            let agent = class == AdmitClass::Agent;
            if s + qs > soft || b + qb > ENTRY_MAX || agent && (d.agents >= AGENT_CMDS_ENTRY as u32 || ad.tokens < 1.0)
            {
                d.open = false;
                break;
            }
            (s, b) = (s + qs, b + qb);
            if agent {
                d.agents += 1;
                ad.tokens -= 1.0;
            }
            take.insert(seq);
        }
        if take.is_empty() {
            return;
        }
        let mut moved = Vec::with_capacity(take.len());
        for c in AdmitClass::ALL {
            let qs = ad.lanes[c.lane()].take_old(|q| take.contains(&q.seq));
            if c == AdmitClass::Agent {
                for q in &qs {
                    ad.agent_bytes -= q.bytes;
                    ad.agent_inline -= q.agent_inline;
                }
            }
            moved.extend(qs);
        }
        moved.sort_unstable_by_key(|q| q.seq);
        self.out.topped.push((moved.len() as u32, d.behind));
        d.batch.extend(moved);
    }

    /// Each command's `decide` sees the fold at `applied` plus the earlier commands of the same entry. The entry is
    /// appended even if every command was refused. Runs only with no entry in flight, so `applied` is the log's end.
    /// `ids` are the `DepsWanted` whose reads `deps` holds, reported as `DepsUsed` with the entry's index.
    fn decide(&mut self, batch: Vec<Queued<C>>, deps: Vec<(ChainId, C::Dep)>, ids: SmallVec<[u64; 2]>) {
        debug_assert!(self.lead.as_ref().is_some_and(|l| l.pending.is_none()), "one entry in flight");
        debug_assert_eq!(self.fold.applied(), self.last(), "decide on the fold at the log's end");
        let deps = &deps[..];
        let mut events = Vec::new();
        let mut requests = SmallVec::new();
        let mut decided = Vec::with_capacity(batch.len());
        let mut tiers = TierSet::NONE;
        let mut format = Format { family: 0, version: 0 };
        let extend = self.take_extend();
        {
            let mut s = self.fold.fold.scratch_at(self.now, TierSet(u8::MAX));
            if let Some(x) = extend {
                s.extend(x);
            }
            // DEMOTE_PER_MIN at this entry, from the replicated window, so a new leader counts what its predecessor
            // committed. The leader's held and refused requests are left out: they are not in the log.
            // The entry's earlier commands count too (`in_entry`), as `decide` sees the fold plus them.
            let from = Timestamp(self.now.0.saturating_sub(DEMOTE_WINDOW.as_millis() as u64));
            let mut in_entry: BTreeMap<crate::admit::ActorKey, u32> = BTreeMap::new();
            for q in batch {
                tiers = tiers.union(q.cmd.tiers());
                format = format.max(q.cmd.format());
                if let Some(a) = q.cmd.actor() {
                    let earlier = in_entry.get(&a).copied().unwrap_or(0);
                    s.set_actor_writes(self.fold.actor_writes(a, from, self.now) + earlier);
                }
                match q.cmd.decide(&s, deps) {
                    Ok((evs, out)) => {
                        let start = events.len() as u32;
                        for e in &evs {
                            if let Some(a) = C::Fold::write_mark(e).and_then(|m| m.actor) {
                                *in_entry.entry(a).or_insert(0) += 1;
                            }
                            s.apply(e);
                        }
                        events.extend(evs);
                        requests.push((q.id, start..events.len() as u32));
                        decided.push((q.seq, q.unstored(), Ok(out)));
                    }
                    Err(err) => {
                        decided.push((q.seq, q.unstored(), Err(err)));
                        self.out.refused.push((self.last().next(), q.id, q.cmd));
                    }
                }
            }
        }
        let last = self.last();
        let e = LogEntry {
            term: self.state.term,
            index: last.next(),
            prev: self.digest_at(last).expect("tail digest"),
            format,
            tiers,
            at: self.now,
            requests,
            extend,
            body: EntryBody::Events(events),
        };
        let index = e.index;
        self.push_entry(e);
        if !ids.is_empty() {
            self.out.deps_used.push(DepsUsed { ids, index });
        }
        self.lead.as_mut().expect("leading").pending = Some((index, decided));
        self.broadcast(false);
        self.advance_commit();
    }

    fn append_id_block(&mut self, block: IdBlock) {
        let last = self.last();
        let e = LogEntry {
            term: self.state.term,
            index: last.next(),
            prev: self.digest_at(last).expect("tail digest"),
            format: crate::RAFT_FORMAT,
            tiers: TierSet::CORE,
            at: self.now,
            requests: SmallVec::new(),
            extend: self.take_extend(),
            body: EntryBody::IdBlock(block),
        };
        let index = e.index;
        self.push_entry(e);
        let lead = self.lead.as_mut().expect("leading");
        lead.pending = Some((index, Vec::new()));
        lead.block_pending = Some((index, block));
        self.broadcast(false);
        self.advance_commit();
    }

    fn append_record(&mut self, id: RequestId, format: Format, events: Vec<crate::Ev<C>>) {
        let last = self.last();
        let n = u32::try_from(events.len()).expect("events fit an entry");
        let tiers = events.iter().fold(TierSet::NONE, |t, ev| t.union(C::Fold::tiers(ev)));
        let e = LogEntry {
            term: self.state.term,
            index: last.next(),
            prev: self.digest_at(last).expect("tail digest"),
            format,
            tiers,
            at: self.now,
            requests: smallvec::smallvec![(id, 0..n)],
            extend: self.take_extend(),
            body: EntryBody::Events(events),
        };
        let index = e.index;
        self.push_entry(e);
        let lead = self.lead.as_mut().expect("leading");
        lead.pending = Some((index, Vec::new()));
        lead.record_pending = Some((index, id));
        self.broadcast(false);
        self.advance_commit();
    }

    fn resolve(&mut self, seq: u64, id: Option<RequestId>, unstored: usize, result: Outcome<C>) {
        let ticket = self.ticket(seq);
        let lead = self.lead.as_mut().expect("leading");
        lead.admission.resolved(seq);
        match id {
            Some(id) => {
                lead.by_request.remove(&id);
            }
            None => lead.by_request.retain(|_, s| *s != seq),
        }
        if let Some((_, b, _)) = lead.done.insert(seq, (self.now, unstored, result.clone())) {
            lead.done_bytes -= b;
        }
        lead.done_bytes += unstored;
        self.prune_done();
        self.out.resolutions.push(Resolution::Ticket { ticket, result });
    }

    /// Drops the oldest resolved tickets past DONE_KEEP, DONE_BYTES, or OUTCOME_KEEP (`Lead::done`).
    fn prune_done(&mut self) {
        let Some(lead) = self.lead.as_mut() else { return };
        let old = Timestamp(self.now.0.saturating_sub(OUTCOME_KEEP.as_millis() as u64));
        while let Some((_, (at, b, _))) = lead.done.first_key_value()
            && (lead.done.len() > DONE_KEEP || lead.done_bytes > DONE_BYTES || *at < old)
        {
            lead.done_bytes -= *b;
            lead.done.pop_first();
        }
    }

    /// The host reported the call for `round` to `to` failed. If it is the outstanding call, retry at once, at most
    /// COMMIT_RETRIES times in a row.
    pub(super) fn append_failed(&mut self, to: SiteId, round: u64) {
        let Some(lead) = self.lead.as_mut() else { return };
        let latest = lead.sent.get(&to).is_some_and(|s| s.0 == round);
        let piped = lead.pipe.get(&to).is_some_and(|p| p.iter().any(|(r, _)| *r == round));
        if !latest && !piped {
            return;
        }
        // The calls after a failed one cannot be stored behind it: start again from `next`.
        lead.sent.remove(&to);
        lead.pipe.remove(&to);
        let n = lead.retries.entry(to).or_insert(0);
        *n += 1;
        if *n <= crate::COMMIT_RETRIES {
            self.send_append(to, true);
        } else {
            // `tick` sends again after CALL_DEADLINE, as for a call with no answer.
            let now = self.now;
            self.lead.as_mut().expect("leading").sent.insert(to, (round, now));
        }
    }

    pub(super) fn broadcast_all(&mut self) {
        self.broadcast(true);
    }

    pub(super) fn pump_after_transfer(&mut self) {
        self.pump();
    }

    /// Sends AppendEntries to every follower. `force` starts a new round even where a call is outstanding.
    fn broadcast(&mut self, force: bool) {
        for p in self.targets() {
            self.send_append(p, force);
        }
    }

    /// Before the answer to a learner's read-index batch at `index`: when it was not yet sent the entries through
    /// `index`, an AppendEntries that leaves ahead of the `ReadIndexRes`, in the same response while the learner calls
    /// this member (`design/DESIGN.md`, "Follower read"). Without it a far learner's entries trailed its answer by one
    /// or two more of its calls: p50 574 to 668 ms and p99 1.9 to 4.5 s on weur and apac at 500 agents, against a
    /// read-index round trip of p99 476 to 639 ms. With calls in flight it follows them, within PIPELINE. With none,
    /// because a refusal or failure emptied the pipeline, it goes from the first entry the learner has not
    /// acknowledged: a far learner's calls end in any order, one that overtook another was refused, and with nothing
    /// sent until CALL_DEADLINE's resend one apac learner a run waited p99 4.8 to 7.8 s for its entries. Re-sending
    /// from that entry on every read instead took the leader to 122 MiB and lost its leadership. Not to a voter, which
    /// is sent every entry as it is cut, nor on a lossy link (F67 rule 13); `tests/am1/results/narrow-reads-3.md`,
    /// "Where a far learner's wait goes".
    pub(super) fn append_for_read(&mut self, peer: SiteId, index: Index) {
        if quorum::is_voter(self.log_config(), peer) || self.link_lossy(peer) {
            return;
        }
        // Up to PIPELINE calls, so a learner behind by more than one call's bytes catches up: with one per read, an
        // apac learner got one to two entries per 350 ms round trip, each call held to APPEND_BYTES_MAX, while the
        // leader committed more, and stayed about 12 entries behind (gate10, `tests/nr3/rows/trace-gate10.json.gz`).
        for _ in 0..PIPELINE {
            let Some(lead) = self.lead.as_ref() else { return };
            let next = lead.next.get(&peer).copied().unwrap_or(self.last().next());
            // A catch-up read from storage, or an install, is out or due: `tick` sends it.
            if next <= self.base.index || self.install.as_ref().is_some_and(|s| Index(next.0 - 1) < s.through) {
                return;
            }
            match lead.pipe.get(&peer).map(|p| (p.len(), p.last().map(|l| l.1))) {
                Some((n, Some(through))) if through < index && n < PIPELINE => self.append_to(peer, false, true),
                Some((_, Some(_))) => return,
                _ if lead.matched(peer) < index => self.send_append(peer, true),
                _ => return,
            }
        }
    }

    /// Sends AppendEntries to `peer`. With a call outstanding and no `force`, a voter whose link is not lossy is sent
    /// only the entries after its outstanding calls, while fewer than PIPELINE are out (`Lead::pipe`).
    fn send_append(&mut self, peer: SiteId, force: bool) {
        self.append_to(peer, force, false);
    }

    /// `send_append`, with `read` pipelining a learner as a voter (`append_for_read`).
    fn append_to(&mut self, peer: SiteId, force: bool, read: bool) {
        let (term, me, boot, now, commit) = (self.state.term, self.cfg.me, self.boot, self.now, self.state.commit);
        let last = self.last();
        let base = self.base.index;
        let voter = read || quorum::is_voter(self.log_config(), peer);
        let lossy = self.link_lossy(peer);
        let Some(lead) = self.lead.as_mut() else { return };
        let next = *lead.next.entry(peer).or_insert(last.next());
        let after = if !force && lead.sent.contains_key(&peer) {
            let pipe = lead.pipe.get(&peer).map_or(&[][..], Vec::as_slice);
            match pipe.last() {
                Some(&(_, through))
                    if voter
                        && !lossy
                        && pipe.len() < PIPELINE
                        && through < last
                        && through >= base
                        && through.next() >= next =>
                {
                    Some(through)
                }
                _ => return,
            }
        } else {
            None
        };
        let prev = after.unwrap_or(Index(next.0 - 1));
        let install = after.is_none() && self.install.as_ref().is_some_and(|s| prev < s.through);
        // The entries this call carries and their bytes; a catch-up from storage carries at most APPEND_BYTES_MAX past
        // its first entry, which `catchup` counts once the host read them.
        let (window, bytes) = if install {
            (0..0, 0)
        } else if prev < base {
            (0..0, APPEND_BYTES_MAX)
        } else {
            // Subtract the absolute indexes in u64 first: on wasm32 a cast to usize would truncate them. The
            // differences are bounded by the in-memory window, so they fit.
            let from = (prev.0 - base.0) as usize;
            let to = ((last.0 - base.0) as usize).min(from + CATCHUP_MAX);
            let n = append_batch(&self.log[from..to]);
            (from..from + n, self.sizes[from..from + n].iter().sum())
        };
        if !self.out_allows(peer, bytes) {
            return;
        }
        let lead = self.lead.as_mut().expect("leading");
        lead.matched.entry(peer).or_insert(Index(0));
        let transfer = lead.transfer.as_ref().map(|t| (t.to, t.id));
        lead.round += 1;
        let round = lead.round;
        lead.sent.insert(peer, (round, now));
        if after.is_none() {
            lead.pipe.remove(&peer);
            if self.send_install(peer, prev, round) {
                return;
            }
        }
        if prev < self.base.index {
            // Below the in-memory window: the host reads the entries from storage and calls `catchup`.
            let through = Index(self.base.index.0.min(prev.0 + CATCHUP_MAX as u64));
            self.out.catchup.push(super::Catchup { to: peer, round, from: prev, through });
            return;
        }
        let entries = self.log[window].to_vec();
        let through = Index(prev.0 + entries.len() as u64);
        self.lead.as_mut().expect("leading").pipe.entry(peer).or_default().push((round, through));
        if bytes > 0 {
            self.calls_out.insert((peer, round), (now, bytes));
        }
        let req = AppendReq {
            term,
            leader: me,
            boot,
            round,
            sent_at: now,
            prev_index: prev,
            prev_term: self.term_at(prev).expect("prev is held"),
            prev_digest: self.digest_at(prev).expect("prev is held"),
            entries,
            commit,
            transfer,
            lead_seq: self.state.lead_seq,
        };
        self.lead.as_mut().expect("leading").telling.insert(peer, (round, commit, now));
        self.send(peer, Message::Append(req));
    }

    /// Whether a call of `bytes` to `peer` fits PEER_OUT_MAX, LEARNERS_OUT_MAX, and OUT_MAX beside the calls alive,
    /// dropping those older than OUT_KEEP first. A voter with none alive always gets one.
    fn out_allows(&mut self, peer: SiteId, bytes: usize) -> bool {
        let old = Timestamp(self.now.0.saturating_sub(OUT_KEEP.as_millis() as u64));
        self.calls_out.retain(|_, (at, _)| *at >= old);
        if bytes == 0 {
            return true;
        }
        let learner = |s: SiteId| graftyard_types::learner_slot(s).is_some();
        let (mut mine, mut learners, mut total) = (0, 0, 0);
        for (&(s, _), &(_, b)) in &self.calls_out {
            total += b;
            mine += if s == peer { b } else { 0 };
            learners += if learner(s) { b } else { 0 };
        }
        if !learner(peer) && mine == 0 {
            return true;
        }
        mine + bytes <= PEER_OUT_MAX
            && (!learner(peer) || learners + bytes <= LEARNERS_OUT_MAX)
            && total + bytes <= OUT_MAX
    }

    /// The stored entries an `Out::catchup` asked for, `max(from, 1)..=through` in order: the entry at `from` gives the
    /// round's `prev`. Sends them to the follower as the round it named, if this member still leads that round.
    pub fn catchup(&mut self, now: Timestamp, c: super::Catchup, entries: Vec<LogEntry<crate::Ev<C>>>) -> Ready<C, ()> {
        self.begin(now);
        let (term, me, boot, commit, lead_seq) =
            (self.state.term, self.cfg.me, self.boot, self.state.commit, self.state.lead_seq);
        let ok = self.role == Role::Leader
            && self.lead.as_ref().is_some_and(|l| l.sent.get(&c.to).is_some_and(|s| s.0 == c.round));
        let mut entries = entries.into_iter();
        let (prev_term, prev_digest) = if c.from == Index(0) {
            (crate::Term(0), crate::PREV_ZERO)
        } else {
            match entries.next() {
                Some(e) if e.index == c.from => (e.term, crate::digest(&e)),
                _ => return self.finish(()),
            }
        };
        let mut entries: Vec<_> = entries.collect();
        entries.truncate(append_batch(&entries));
        let chained = entries.iter().enumerate().all(|(k, e)| e.index.0 == c.from.0 + 1 + k as u64);
        if ok && chained && !entries.is_empty() {
            let lead = self.lead.as_mut().expect("leading");
            let transfer = lead.transfer.as_ref().map(|t| (t.to, t.id));
            lead.sent.insert(c.to, (c.round, now));
            let bytes = entries.iter().map(|e| crate::encode_entry(e).len()).sum();
            self.calls_out.insert((c.to, c.round), (now, bytes));
            let req = AppendReq {
                term,
                leader: me,
                boot,
                round: c.round,
                sent_at: now,
                prev_index: c.from,
                prev_term,
                prev_digest,
                entries,
                commit,
                transfer,
                lead_seq,
            };
            self.lead.as_mut().expect("leading").telling.insert(c.to, (c.round, commit, now));
            self.send(c.to, Message::Append(req));
        }
        self.finish(())
    }

    pub(super) fn on_append_res(&mut self, from: SiteId, res: AppendRes) {
        // Any answer ends its call, in any role or term: its bytes stop counting (`out_allows`).
        if res.boot == self.boot {
            self.calls_out.remove(&(from, res.round));
        }
        if res.term > self.state.term {
            self.step_down(res.term);
            return;
        }
        if self.role != Role::Leader || res.term != self.state.term || res.boot != self.boot {
            return;
        }
        // Any answer is a call that succeeded, for `STEP_DOWN`.
        self.record_call(from, res.round, true, false, res.sent_at);
        let last = self.last();
        let lead = self.lead.as_mut().expect("leading");
        lead.retries.remove(&from);
        lead.heard.insert(from, self.now);
        lead.health.peer_quarantine.insert(from, (res.stepped_down_at, res.boots));
        if lead.sent.get(&from).is_some_and(|s| s.0 == res.round) {
            lead.sent.remove(&from);
        }
        if let Some(p) = lead.pipe.get_mut(&from) {
            p.retain(|(r, _)| *r != res.round);
            // A refusal, stale or not, means the calls after it may not be stored: pipeline again only from an ack.
            if !res.ok || p.is_empty() {
                lead.pipe.remove(&from);
            }
        }
        if res.round < lead.min_round || lead.member_from.get(&from).is_some_and(|r| res.round < *r) {
            // Sent before the leader truncated its uncommitted tail, so `matched` may name entries it no longer holds;
            // or sent to the member this address named before, whose log is not the new member's.
            return;
        }
        let matched = lead.matched.entry(from).or_insert(Index(0));
        if (res.ok || res.refused.is_some()) && res.matched > *matched {
            *matched = res.matched;
            lead.matched_at.insert(from, self.now);
        }
        let matched = *matched;
        let next = lead.next.entry(from).or_insert(last.next());
        if res.ok {
            *next = (*next).max(matched.next());
            // The latest round's answer: the follower set its commit to the lower of the round's and what it matched.
            if let Some(&(round, commit, _)) = lead.telling.get(&from)
                && round == res.round
            {
                let told = lead.told.entry(from).or_insert(Index(0));
                *told = (*told).max(commit.min(res.matched));
            }
            if res.sent_at >= lead.lease_floor {
                let a = lead.acked_at.entry(from).or_insert(res.sent_at);
                *a = (*a).max(res.sent_at);
            }
            if res.promise && res.sent_at >= lead.lease_floor {
                let p = lead.promised_at.entry(from).or_insert(res.sent_at);
                *p = (*p).max(res.sent_at);
            }
            lead.refusals.remove(&from);
            if let Some(i) = self.state.cannot_follow.iter().position(|s| *s == from) {
                self.state.cannot_follow.remove(i);
                self.refuser_release.remove(&from);
                self.dirty = true;
            }
            self.transfer_ack(from, res.round);
            self.recompute_lease();
            self.advance_commit();
            self.after_ack();
            self.classify_refusals();
            if self.lead.as_ref().is_some_and(|l| l.next.get(&from).is_some_and(|n| *n <= self.last())) {
                self.send_append(from, false);
            }
        } else if let Some(refusal) = res.refused {
            *next = (*next).max(matched.next());
            // A refuser in `cannot_follow` whose release changed may follow again.
            if self.refuser_release.get(&from).is_some_and(|r| *r != refusal.release) {
                self.refuser_release.remove(&from);
                if let Some(i) = self.state.cannot_follow.iter().position(|s| *s == from) {
                    self.state.cannot_follow.remove(i);
                    self.dirty = true;
                }
            }
            if !self.state.cannot_follow.contains(&from) {
                let now = self.now;
                let lead = self.lead.as_mut().expect("leading");
                let at = lead.refusals.get(&from).map_or(now, |r| r.1);
                lead.refusals.insert(from, (refusal, at));
                self.classify_refusals();
            }
        } else if res.prev_index.next() == *next {
            // A refusal for the current probe: back up, and probe again at once only if `next` moved. Otherwise
            // `tick` retries after CALL_DEADLINE. A stale refusal is ignored.
            let was = *next;
            *next = Index(res.prev_index.0.min(res.last.0 + 1).max(1)).max(matched.next());
            if *next < was {
                self.send_append(from, true);
            }
        }
    }

    /// The leader's side of `verify` refusals (`design/DESIGN.md`, `verify`). A refusal of an uncommitted entry is
    /// classified once its round settles: the entry commits, every follower refused it, or CAMPAIGN_DEADLINE passed.
    /// - Uncommitted and refused by an older follower: truncate the uncommitted tail, persist `no_lead_until`, and
    ///   transfer, to the refuser when it qualifies.
    /// - Uncommitted and refused by both followers on this release: its own defect. Truncate the uncommitted tail,
    ///   alarm, and keep leading.
    /// - Otherwise (same or newer release, or a committed entry): keep leading, add the refuser to `cannot_follow`,
    ///   and alarm.
    pub(super) fn classify_refusals(&mut self) {
        let Some(lead) = &self.lead else { return };
        if lead.refusals.is_empty() {
            return;
        }
        let commit = self.state.commit;
        let mine = self.cfg.release;
        let peers = self.peers();
        let first = lead.refusals.values().map(|r| r.1).min().expect("non-empty");
        let index = lead.refusals.values().map(|r| r.0.index).min().expect("non-empty");
        let all = peers.iter().all(|p| lead.refusals.contains_key(p) || self.state.cannot_follow.contains(p));
        let settled = index <= commit || all || self.now >= after(first, CAMPAIGN_DEADLINE);
        if !settled {
            return;
        }
        let refusals = std::mem::take(&mut self.lead.as_mut().expect("leading").refusals);
        // A learner's refusal only alarms: it never votes, so it cannot take the hand-off or prove an own defect.
        let older: Vec<(SiteId, Refusal)> = refusals
            .iter()
            .filter(|(s, (r, _))| peers.contains(s) && r.index > commit && r.release.seq < mine.seq)
            .map(|(s, (r, _))| (*s, r.clone()))
            .collect();
        if let Some((site, refusal)) = older.first().cloned() {
            self.truncate_uncommitted();
            self.state.no_lead_until = Some(mine.id);
            self.dirty = true;
            self.out.alarms.push(Alarm::NoLead { site, refusal });
            self.lead.as_mut().expect("leading").refuser = Some(site);
            self.hand_off(TransferWhy::NoLead);
            return;
        }
        let own: Vec<&Refusal> = refusals
            .iter()
            .filter(|(s, (r, _))| peers.contains(s) && r.index > commit && r.release == mine)
            .map(|(_, (r, _))| r)
            .collect();
        if own.len() >= 2 && own.len() == peers.len() {
            let index = own.iter().map(|r| r.index).min().expect("two");
            self.truncate_uncommitted();
            self.out.alarms.push(Alarm::OwnDefect { index });
            return;
        }
        for (site, (refusal, _)) in refusals {
            if !self.state.cannot_follow.contains(&site) {
                self.state.cannot_follow.push(site);
                self.refuser_release.insert(site, refusal.release);
                self.dirty = true;
            }
            self.out.alarms.push(Alarm::Refused { site, refusal });
        }
    }

    /// Truncates the leader's uncommitted tail. Its tickets answer `NotLeader`, so the edge re-sends each with the
    /// same request id and the next entry decides it again.
    ///
    /// With one entry in flight the tail is the refused entry alone, after the term's committed `Noop`. A leader that
    /// keeps leading (its own defect) appends at that index again in the same term, which is safe only because both
    /// followers refused the entry, so neither holds it. A leader with `no_lead_until` appends nothing more in its term.
    fn truncate_uncommitted(&mut self) {
        let commit = self.state.commit;
        let nl = self.not_leader();
        let lead = self.lead.as_mut().expect("leading");
        let mut seqs: Vec<u64> = Vec::new();
        if let Some((_, decided)) = lead.pending.take() {
            seqs.extend(decided.iter().map(|(s, _, _)| *s));
        }
        if let Some((_, block)) = lead.block_pending.take() {
            self.out.resolutions.push(Resolution::IdBlock { block, result: Err(nl.clone()) });
        }
        if let Some((_, id)) = lead.record_pending.take() {
            self.out.resolutions.push(Resolution::Recorded { id, result: Err(nl.clone()) });
        }
        lead.min_round = lead.round + 1;
        // A new leader instance for followers: a round sent before the truncation carries the refused entry, and one
        // delayed past the replacement's commit at the same index would make a follower delete a committed entry
        // (found by the harsh sweep, seed 3013). Followers refuse an older instance's rounds.
        self.state.lead_seq += 1;
        self.dirty = true;
        for m in lead.matched.values_mut() {
            *m = (*m).min(commit);
        }
        for n in lead.next.values_mut() {
            *n = (*n).min(commit.next());
        }
        // The broadcast below forces a fresh call to every follower, which clears its pipe.
        lead.sent.clear();
        lead.ready_at = lead.ready_at.min(commit);
        for seq in seqs {
            self.resolve(seq, None, 0, Err(CommitError::NotLeader(nl.clone())));
        }
        if self.last() > commit {
            self.truncate_from(commit.next());
        }
        self.broadcast(true);
    }

    /// A leader that must not lead (`no_lead_until`, or its site drained) transfers to a follower that may lead: the
    /// refuser when it qualifies, else the most caught up.
    pub(super) fn hand_off(&mut self, why: TransferWhy) {
        let Some(lead) = &self.lead else { return };
        if lead.transfer.is_some() {
            return;
        }
        let mut to: Vec<SiteId> = self
            .peers()
            .into_iter()
            .filter(|s| self.may_lead_target(*s) && !self.quarantined(*s) && !self.link_refused(*s))
            .collect();
        let refuser = lead.refuser;
        to.sort_by_key(|s| (Some(*s) != refuser, std::cmp::Reverse(lead.matched(*s))));
        if let Some(t) = to.first().copied() {
            let _ = self.start_transfer(t, why, true);
        }
    }

    /// The latest t such that this leader plus the followers that promised on rounds sent at or after t make a
    /// quorum (both majorities under `Joint`); the lease runs LEASE from t.
    fn recompute_lease(&mut self) {
        let t = self.quorum_since(|l| &l.promised_at);
        let lead = self.lead.as_mut().expect("leading");
        if let Some(t) = t {
            lead.lease_end = lead.lease_end.max(after(t, LEASE));
        }
    }

    fn quorum_since(&self, pick: impl Fn(&Lead<C>) -> &BTreeMap<SiteId, Timestamp>) -> Option<Timestamp> {
        let lead = self.lead.as_ref()?;
        let times = pick(lead);
        let mut ts: Vec<Timestamp> = times.values().copied().collect();
        ts.push(self.now);
        ts.sort_unstable_by(|a, b| b.cmp(a));
        let config = self.log_config();
        ts.into_iter().find(|t| {
            let mut yes: BTreeSet<SiteId> = times.iter().filter(|(_, at)| *at >= t).map(|(s, _)| *s).collect();
            yes.insert(self.cfg.me);
            quorum::election(config, &yes)
        })
    }

    /// The commit-advance path: the highest index of this term that the quorum rule holds for. Tickets resolve only
    /// here, so `Acked` is built only after a quorum stored the entry.
    ///
    /// The leader's AppendEntries leave before its own write is durable, so it counts its own copy only through
    /// `own_durable`, which only `self_ack` raises. It also commits nothing above `own_durable`, even when both
    /// followers hold the entry: a reset leader resumes `led_term` with no election, so if it could lose an entry the
    /// followers had committed, it would write another entry at that committed index in the same term. Below
    /// `own_durable` its own copy always counts.
    pub(super) fn advance_commit(&mut self) {
        let Some(lead) = &self.lead else { return };
        let mut n = self.last().min(self.own_durable);
        while n > self.state.commit && n >= lead.commit_floor && self.term_at(n) == Some(self.state.term) {
            let mut acked: BTreeSet<SiteId> = lead.matched.iter().filter(|(_, m)| **m >= n).map(|(s, _)| *s).collect();
            acked.insert(self.cfg.me);
            if quorum::commit(self.log_config(), &acked) {
                let proof = self.proof_for(n, &acked);
                let entries = n.0 - self.state.commit.0;
                self.set_commit(n);
                self.note_commit(entries);
                let lead = self.lead.as_mut().expect("leading");
                lead.proof = Some(proof.clone());
                if let Some((index, _)) = &lead.pending
                    && *index <= n
                {
                    let (index, decided) = lead.pending.take().expect("pending");
                    if !decided.is_empty()
                        && let Some(at) = self.entry_at(index).map(|e| e.at)
                    {
                        let ad = &mut self.lead.as_mut().expect("leading").admission;
                        ad.round_ms = 0.9 * ad.round_ms + 0.1 * self.now.0.saturating_sub(at.0) as f32;
                    }
                    let lead = self.lead.as_mut().expect("leading");
                    if let Some((_, block)) = lead.block_pending.take() {
                        self.out.resolutions.push(Resolution::IdBlock { block, result: Ok(index) });
                    }
                    if let Some((_, id)) = lead.record_pending.take() {
                        self.out.resolutions.push(Resolution::Recorded { id, result: Ok(index) });
                    }
                    for (seq, unstored, result) in decided {
                        self.resolve(seq, None, unstored, outcome::settle(index, &proof, result));
                    }
                }
                self.after_ack();
                self.pump();
                return;
            }
            n = Index(n.0 - 1);
        }
    }

    /// The evidence for a commit at `n` by `acked`: each voter's durable index as this leader counted it and when it
    /// learned it, with the entry's `at` and the time now. Only the voters of the config the commit counted are named:
    /// a learner never makes a write durable, and the edge shows these sites to callers as the write's quorum.
    fn proof_for(&self, n: Index, acked: &BTreeSet<SiteId>) -> QuorumProof {
        let lead = self.lead.as_ref().expect("leading");
        let voters = quorum::voters(self.log_config());
        let acks: Vec<DurableAck> = acked
            .iter()
            .filter(|s| voters.contains(s))
            .map(|s| {
                if *s == self.cfg.me {
                    DurableAck { site: *s, durable: self.own_durable, at: self.own_durable_at }
                } else {
                    let at = lead.matched_at.get(s).copied().unwrap_or(Timestamp(0));
                    DurableAck { site: *s, durable: lead.matched(*s), at }
                }
            })
            .collect();
        let entry_at = self.entry_at(n).map_or(Timestamp(0), |e| e.at);
        QuorumProof::counted(self.cfg.me, n, entry_at, self.now, &acks, &self.cfg.regions)
    }

    /// Marks the leader ready once its term's `Noop` (or its resumed tail) committed and a round reached a quorum,
    /// then serves waiting reads and queued commands.
    pub(super) fn after_ack(&mut self) {
        let Some(lead) = &self.lead else { return };
        if !lead.ready && self.state.commit >= lead.ready_at {
            let started = lead.started;
            // A resumed `led_term` may find its whole log committed already, so no commit advance in this boot built a
            // proof. The followers that store `ready_at` with this leader make one.
            let mut stored: BTreeSet<SiteId> =
                lead.matched.iter().filter(|(_, m)| **m >= lead.ready_at).map(|(s, _)| *s).collect();
            if lead.ready_at <= self.own_durable {
                stored.insert(self.cfg.me);
            }
            let proven = lead.proof.is_some() || quorum::commit(self.log_config(), &stored);
            if proven && self.quorum_since(|l| &l.acked_at).is_some_and(|t| t >= started) {
                let proof = self.proof_for(lead.ready_at, &stored);
                let lead = self.lead.as_mut().expect("leading");
                lead.proof.get_or_insert(proof);
                lead.ready = true;
                self.recompute_lease();
                self.pump();
            }
        }
        self.check_reads();
    }

    /// `fresh`, first half: a leader whose term's `Noop` is committed answers from its fold while `now < lease_end -
    /// DRIFT`, else after one read-index round sent after the read arrived. The grant comes back as a
    /// `Resolution::Read`; `Member::fresh` turns it into the view.
    pub fn read(&mut self, now: Timestamp) -> Ready<C, ReadStart> {
        self.begin(now);
        let r = self.start_read();
        self.finish(r)
    }

    pub(super) fn start_read(&mut self) -> ReadStart {
        self.lead_or_resume()?;
        let id = ReadId(self.next_read);
        self.next_read += 1;
        let now = self.now;
        let lead = self.lead.as_mut().expect("leading");
        let lease = lead.ready
            && lead.transfer.is_none()
            && !self.clock_suspect
            && now < Timestamp(lead.lease_end.0.saturating_sub(DRIFT.as_millis() as u64));
        lead.reads.push((id, now, !lease));
        if !lease {
            self.read_round();
        }
        self.check_reads();
        Ok(id)
    }

    /// A read-index round, to the voters only (a learner's answer counts toward no quorum), for the reads that need
    /// one and arrived after the latest round left. Every grant still comes from a round sent at or after the read
    /// arrived. The next round leaves once the latest is answered, or READ_ROUND_GAP after it left if it is not, so
    /// rounds per voter stay at most one per answer or per gap. Shared per clock reading instead (read-rounds), a
    /// lease-clock floor raised by each newer peer stamp gave reads in one turn new readings, and L0-12 m3fr6's leader
    /// still sent up to 99 rounds to each voter in one millisecond (`tests/m0/results/read-round-gap.md`).
    fn read_round(&mut self) {
        let answered = self.quorum_since(|l| &l.acked_at);
        let now = self.now;
        let own = self.own_clock.unwrap_or(now);
        let Some(lead) = self.lead.as_ref() else { return };
        let last = lead.read_round;
        let waiting = lead.reads.iter().any(|(_, arrived, needs)| *needs && last.is_none_or(|(t, _)| *arrived > t));
        let free =
            last.is_none_or(|(t, o)| own > o && (own >= after(o, READ_ROUND_GAP) || answered.is_some_and(|a| a >= t)));
        if !waiting || !free {
            return;
        }
        self.lead.as_mut().expect("leading").read_round = Some((now, own));
        for p in self.peers() {
            if !self.read_beat(p) {
                self.send_append(p, true);
            }
        }
    }

    /// A read round to a voter that already has pipelined AppendEntries out: an AppendEntries with no entries at the
    /// voter's acknowledged index, which it accepts whatever its calls in flight carry, and whose ack counts in
    /// `acked_at` like any other. It leaves the pipeline and the outstanding call (`sent`) alone, so its answer starts
    /// no re-send, and its failure resets nothing. A forced send instead reset the pipeline and carried the entry in
    /// flight again, at each round: on B0-29 (mwkyk9zg) each entry went 6.4 to 7.8 times to each voter, 84 to 87% of
    /// the leader's AppendEntries and 640 to 940 MB a run (`tests/m0/results/read-beat.md`). False, and the round
    /// sends as before, to a voter with no pipeline (none out, a lossy link, a catch-up, or an install), whose
    /// outstanding call left READ_RESEND ago or more, or whose acknowledged entry is below the in-memory window.
    fn read_beat(&mut self, peer: SiteId) -> bool {
        let (term, me, boot, now, commit) = (self.state.term, self.cfg.me, self.boot, self.now, self.state.commit);
        let base = self.base.index;
        let Some(lead) = self.lead.as_mut() else { return false };
        let matched = lead.matched(peer);
        let fresh = lead.sent.get(&peer).is_some_and(|(_, at)| now < after(*at, READ_RESEND));
        if lead.pipe.get(&peer).is_none_or(Vec::is_empty) || !fresh || matched < base {
            return false;
        }
        let (Some(prev_term), Some(prev_digest)) = (self.term_at(matched), self.digest_at(matched)) else {
            return false;
        };
        let lead = self.lead.as_mut().expect("leading");
        let transfer = lead.transfer.as_ref().map(|t| (t.to, t.id));
        lead.round += 1;
        let req = AppendReq {
            term,
            leader: me,
            boot,
            round: lead.round,
            sent_at: now,
            prev_index: matched,
            prev_term,
            prev_digest,
            entries: Vec::new(),
            commit,
            transfer,
            lead_seq: self.state.lead_seq,
        };
        let lead = self.lead.as_mut().expect("leading");
        lead.telling.insert(peer, (lead.round, commit, now));
        self.send(peer, Message::Append(req));
        true
    }

    fn check_reads(&mut self) {
        let Some(lead) = &self.lead else { return };
        if lead.reads.is_empty() {
            return;
        }
        let now = self.now;
        // A transferring leader serves no `fresh`, and a `no_lead_until` one is handing off.
        let ready = lead.ready && lead.transfer.is_none() && !self.no_lead();
        let lease = ready && now < Timestamp(lead.lease_end.0.saturating_sub(DRIFT.as_millis() as u64));
        let round = self.quorum_since(|l| &l.acked_at);
        let reads = std::mem::take(&mut self.lead.as_mut().expect("leading").reads);
        let mut keep = Vec::new();
        for (id, arrived, needs_round) in reads {
            let round_ok = round.is_some_and(|t| t >= arrived);
            let result = if ready && (if needs_round { round_ok } else { lease || round_ok }) {
                Ok(ReadGrant {
                    term: self.state.term,
                    boot: self.boot,
                    step: self.step,
                    index: self.fold.applied(),
                    floor: self.fold.applied(),
                    via_round: needs_round || !lease,
                    remote: false,
                })
            } else if now >= after(arrived, CAMPAIGN_DEADLINE) {
                Err(self.not_leader())
            } else {
                keep.push((id, arrived, needs_round));
                continue;
            };
            self.read_resolved(id, result);
        }
        self.lead.as_mut().expect("leading").reads = keep;
        self.read_round();
    }

    /// `stale()`: this member's fold at `applied`, on any role, with no grant. The caller holds the flag that allows it.
    pub fn stale(&self) -> crate::Stale<'_, C::Fold> {
        crate::Stale { fold: &self.fold.fold, index: self.fold.applied() }
    }

    /// `fresh`, second half: the view a grant allows, only before the member's next input. A grant from
    /// `read_index` gives a follower's or learner's fold.
    pub fn fresh(&self, grant: &ReadGrant) -> Result<Fresh<'_, C::Fold>, NotLeader> {
        let ok = (self.role == Role::Leader || grant.remote)
            && grant.term == self.state.term
            && grant.boot == self.boot
            && grant.step == self.step
            && grant.index == self.fold.applied();
        if ok { Ok(Fresh { fold: &self.fold.fold, index: grant.index }) } else { Err(self.not_leader()) }
    }

    /// The member's fold now, for a read granted in an earlier input of this boot: every applied entry is committed,
    /// and within a boot the applied index never moves back (an install only moves it forward), so this state is at or
    /// above the grant's floor (`ReadGrant::floor`) and holds every entry acknowledged before the read arrived, as the
    /// grant's own state did. A host that could not answer at the grant (an answer whose build was not due) answers
    /// here later without another read-index round. Refused after the member restarted (`boot`).
    pub fn fresh_after(&self, grant: &ReadGrant) -> Result<Fresh<'_, C::Fold>, NotLeader> {
        if grant.boot != self.boot {
            return Err(self.not_leader());
        }
        Ok(Fresh { fold: &self.fold.fold, index: self.fold.applied() })
    }

    /// The `extend` of the next entry this leader appends (`design/DESIGN.md`, Shape, `ExtendLeases`): with a stall
    /// recorded in `stalled_since`, `since` is the later of the marker and the `at` of the last entry the log holds, so
    /// no lease that entry already judged expired is revived, and `by` runs to now. None while an unapplied entry in
    /// the log already carries one, so one stall moves a lease once, and None while `by` is under `EXTEND_MIN`: the
    /// entry then carries no `extend`, and its commit clears the marker (`set_commit`), so a sub-floor stall moves no
    /// lease. The floor compares the whole stall from `since`, not the last gap.
    fn take_extend(&self) -> Option<ExtendLeases> {
        let marker = self.state.stalled_since?;
        let applied = self.fold.applied();
        if self.log.iter().any(|e| e.index > applied && e.extend.is_some()) {
            return None;
        }
        let last = self.log.last().map_or(Timestamp(0), |e| e.at);
        let since = marker.max(last);
        let by = std::time::Duration::from_millis(self.now.0.saturating_sub(since.0));
        (by >= crate::EXTEND_MIN).then_some(ExtendLeases { since, by })
    }

    pub(super) fn tick_leader(&mut self) {
        let Some(lead) = &self.lead else { return };
        let now = self.now;
        // A stall marker: this leader's entry in flight that no quorum acknowledged within CALL_DEADLINE. Like every
        // marker it rides the member's next state write (`mark_stall`).
        if let Some((index, _)) = &lead.pending
            && let Some(at) = self.entry_at(*index).map(|e| e.at)
            && *index > self.state.commit
            && now >= after(at, CALL_DEADLINE)
        {
            self.state.stalled_since = Some(self.state.stalled_since.map_or(at, |m| m.min(at)));
        }
        let Some(lead) = &self.lead else { return };
        // A leader that has not served within one round's deadline gives up, as a resumed `led_term` does after its
        // round fails. It keeps `led_term`, so a later request may try to resume again.
        if !lead.ready && now >= after(lead.started, CAMPAIGN_DEADLINE) {
            self.end_leadership();
            return;
        }
        // Check quorum (F67): a leader that no quorum answered for ELECTION_AFTER stops leading and gives up
        // `led_term`. Its appends still reached followers whose answers were lost (a half-hung site), so they kept
        // renewing their promises to a leader that could neither commit, serve a read, nor finish a transfer, and no
        // follower could campaign; stopping lets those promises lapse.
        let demand = lead.pending.is_some() || !lead.reads.is_empty() || lead.transfer.is_some();
        let since = match (demand, lead.waiting_since) {
            (false, _) => None,
            (true, None) => Some(now),
            (true, Some(t)) => Some(t),
        };
        self.lead.as_mut().expect("leading").waiting_since = since;
        let Some(lead) = &self.lead else { return };
        let heard = self.quorum_since(|l| &l.heard).unwrap_or(lead.started).max(lead.started);
        if lead.ready && since.is_some_and(|w| now >= after(w.max(heard), crate::ELECTION_AFTER)) {
            self.state.led_term = None;
            self.dirty = true;
            self.end_leadership();
            return;
        }
        let (last, commit) = (self.last(), self.state.commit);
        // A transfer keeps sending rounds that carry its id, so the other follower accepts one.
        let waiting = !lead.ready || !lead.reads.is_empty() || lead.transfer.is_some();
        let due: Vec<SiteId> = self
            .targets()
            .into_iter()
            .filter(|p| {
                let gap = if self.state.cannot_follow.contains(p) { CANNOT_FOLLOW_RETRY } else { CALL_DEADLINE };
                let behind = lead.next.get(p).is_none_or(|n| *n <= last);
                let idle = lead.sent.get(p).is_none_or(|(_, at)| now >= after(*at, gap));
                // A follower that holds every entry but not the latest commit (R29): one round once the last round
                // to it is CALL_DEADLINE old, so a busy group's next entry carries the commit instead.
                let untold = lead.told.get(p).is_none_or(|t| *t < commit)
                    && lead.telling.get(p).is_none_or(|(_, _, at)| now >= after(*at, gap));
                idle && (behind || waiting || untold)
            })
            .collect();
        for p in due {
            self.send_append(p, true);
        }
        self.check_reads();
        self.expire_agents();
        self.prune_done();
        self.pump();
    }
}
