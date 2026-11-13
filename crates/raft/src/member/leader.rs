//! The leader: group commit, `decide`, replication, the commit-advance path, leases, read-index, and tickets.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use graftyard_types::{ChainId, IdBlock, SiteId, Timestamp};
use smallvec::SmallVec;

use crate::admit::{
    AGENT_FLOOR, AGENT_INLINE_ENTRY, AGENT_QUEUE_MAX, AGENT_SHARE, AGENT_WAIT, ActorKey, Admit, AdmitClass, AdmitError,
    DEMOTE_PER_MIN, DEMOTE_WINDOW, HOT_PUSHES, HOT_WINDOW, INLINE_MAX_HOT, Lane, PRESSURE_AGE, retry_after_s,
};
use crate::fold::{Command, Fold, Scratch};
use crate::outcome::{self, Acked};
use crate::quorum::{self, DurableAck, QuorumProof};
use crate::read::{Fresh, ReadGrant};
use crate::types::{
    Admitted, CallError, CommitError, EntryBody, ExtendLeases, Format, GroupConfig, Index, LogEntry, Next, NotLeader,
    RequestId, Ticket, TierSet,
};
use crate::{CALL_DEADLINE, COMMAND_MAX, DRIFT, ENTRY_MAX, LEASE, after};

use super::election::CAMPAIGN_DEADLINE;
use super::{
    AdmissionView, Alarm, AppendReq, AppendRes, ByRequest, DepsWanted, IdBlockError, Member, Message, ReadId,
    ReadStart, Ready, ReconfigError, Refusal, Resolution, Role,
};

/// Entries per AppendEntries when a follower is behind.
const CATCHUP_MAX: usize = 64;
/// Encoded bytes per AppendEntries beyond its first entry. With the count cap alone, 64 entries of up to ENTRY_MAX
/// each could go in one call (up to 96 MB), which no member answers within CALL_DEADLINE, so a follower behind on a
/// repository with inline packs would be sent the same call on every retry (`tests/m1/results/m1-wp8.md`, t-256).
/// One entry always goes, so an entry up to ENTRY_MAX still replicates.
pub const APPEND_BYTES_MAX: usize = 1_500_000;

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
/// How often a follower in `cannot_follow` is sent to again.
const CANNOT_FOLLOW_RETRY: Duration = Duration::from_secs(5);
/// Resolved tickets kept for a late `outcome` call.
const DONE_KEEP: usize = 4096;

struct Queued<C> {
    seq: u64,
    id: RequestId,
    cmd: C,
    /// Its encoded length, and the Agent-class inline pack bytes it carries (0 for every other class).
    bytes: usize,
    agent_inline: usize,
    at: Timestamp,
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
    /// The commit round of an `Events` entry, and the encoded bytes of an admitted command, as moving averages.
    round_ms: f32,
    cmd_bytes: f32,
    /// When the last `Events` entry was cut.
    cut_at: Timestamp,
    /// Commands this leadership admitted with an actor and has not resolved, per actor: they count toward
    /// DEMOTE_PER_MIN beside the committed ones in the fold.
    pending_by: BTreeMap<ActorKey, u32>,
    seq_actor: BTreeMap<u64, ActorKey>,
    /// Writes this leadership refused as Agent, per actor, by time: they never commit, so the fold cannot count them,
    /// and without them a demoted actor whose writes are refused falls back under DEMOTE_PER_MIN and is classed Human
    /// again. Memory only, so a new leader counts only what committed [inference].
    refused_by: BTreeMap<ActorKey, VecDeque<Timestamp>>,
}

/// The round a fresh leader assumes before it has measured one [guess: B0-01's commit p50 on `Tri`].
const ROUND_PRIOR_MS: f32 = 250.0;
/// No capacity estimate uses a round shorter than this [guess, the M0-WP9 prototype's floor].
const ROUND_MIN_MS: f32 = 50.0;
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
            pending_by: BTreeMap::new(),
            seq_actor: BTreeMap::new(),
            refused_by: BTreeMap::new(),
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
    fn capacity(&self, fixed: Option<f32>) -> f32 {
        fixed.unwrap_or_else(|| {
            let per_entry = ENTRY_MAX as f32 / self.cmd_bytes.max(CMD_BYTES_MIN);
            per_entry * 1000.0 / self.round_ms.max(ROUND_MIN_MS)
        })
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

type Decided<C> = (u64, Result<<C as Command>::Out, <C as Command>::Err>);
type Outcome<C> = Result<Acked<<C as Command>::Out>, CommitError<<C as Command>::Err>>;

pub(super) struct Lead<C: Command> {
    started: Timestamp,
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
    /// The outstanding call to each follower.
    sent: BTreeMap<SiteId, (u64, Timestamp)>,
    round: u64,
    admission: Admission<C>,
    seq: u64,
    /// No commit below this index: the second `Noop` of a resumed `led_term` (`become_leader`), else 0.
    commit_floor: Index,
    /// Request ids queued, being decided, or in flight, so a re-send joins the ticket.
    by_request: BTreeMap<RequestId, u64>,
    deciding: Option<(u64, Vec<Queued<C>>)>,
    next_deps: u64,
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
    /// The latest commit proof this leadership counted.
    proof: Option<QuorumProof>,
    done: BTreeMap<u64, Outcome<C>>,
    /// Replies to rounds below this are ignored: the leader truncated its uncommitted tail after they were sent, so
    /// their `matched` may name entries it no longer holds.
    pub(super) min_round: u64,
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
            ready_at: Index(0),
            ready: false,
            next: peers.iter().map(|p| (*p, next)).collect(),
            matched: peers.iter().map(|p| (*p, Index(0))).collect(),
            matched_at: BTreeMap::new(),
            promised_at: BTreeMap::new(),
            acked_at: BTreeMap::new(),
            lease_end: Timestamp(0),
            sent: BTreeMap::new(),
            round: 0,
            admission: Admission::new(self.now),
            seq: 0,
            commit_floor: Index(0),
            by_request: BTreeMap::new(),
            deciding: None,
            next_deps: 0,
            pending: None,
            blocks: VecDeque::new(),
            block_pending: None,
            records: VecDeque::new(),
            record_pending: None,
            reads: Vec::new(),
            proof: None,
            done: BTreeMap::new(),
            min_round: 0,
            transfer: None,
            health: super::health::LeaderHealth::default(),
            refusals: BTreeMap::new(),
            lease_floor: Timestamp(0),
            refuser: None,
            retries: BTreeMap::new(),
            waiting_since: None,
            heard: BTreeMap::new(),
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
            .chain(lead.deciding.iter().flat_map(|(_, b)| b.iter().map(|q| q.seq)))
            .chain(lead.pending.iter().flat_map(|(_, d)| d.iter().map(|(s, _)| *s)));
        let tickets: Vec<Ticket> = seqs.map(|s| self.ticket(s)).collect();
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
        if !lead.ready || lead.pending.is_some() || lead.deciding.is_some() || lead.transfer.is_some() {
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
        let (a, b) = (names(cur), names(&next));
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
        let added: Vec<SiteId> = b.keys().filter(|s| !a.contains_key(s)).copied().collect();
        self.push_entry(e);
        // A site that comes back names a new member: nothing the old member on that site answered may count for it,
        // or its old `matched` would count toward a quorum the new member is not in. It starts from the leader's
        // next index and backs up from its answers.
        let next_index = self.last();
        let lead = self.lead.as_mut().expect("leading");
        for p in added {
            lead.next.insert(p, next_index);
            lead.matched.insert(p, Index(0));
            lead.matched_at.remove(&p);
            lead.sent.remove(&p);
            lead.promised_at.remove(&p);
            lead.acked_at.remove(&p);
            lead.retries.remove(&p);
            lead.refusals.remove(&p);
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

    /// What admission sees now, as leader: capacity, queued agent commands and bytes, whether the group is hot and
    /// under pressure. `None` on a follower.
    pub fn admission(&self) -> Option<AdmissionView> {
        let lead = self.lead.as_ref()?;
        let ad = &lead.admission;
        let cap = ad.capacity(self.admit_capacity);
        Some(AdmissionView {
            capacity: cap,
            agent_queued: ad.agent().len(),
            agent_bytes: ad.agent_bytes,
            queued: ad.lanes.iter().map(Lane::len).sum(),
            hot: self.hot(),
            pressure: self.pressure(cap),
            retry_after_s: retry_after_s(ad.agent().len(), cap),
        })
    }

    /// Pushes a second over the last HOT_WINDOW of committed entries, above HOT_PUSHES.
    fn hot(&self) -> bool {
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
        let cap = ad.capacity(self.admit_capacity);
        let full = || AdmitError::Full { retry_after_s: retry_after_s(ad.agent().len(), cap) };
        if agent && !a.placement_ok && a.inline > INLINE_MAX_HOT {
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
                let now = self.now;
                let lead = self.lead.as_mut().expect("leading");
                lead.seq += 1;
                let seq = lead.seq;
                lead.by_request.insert(id, seq);
                let ad = &mut lead.admission;
                ad.cmd_bytes = 0.98 * ad.cmd_bytes + 0.02 * len as f32;
                let agent_inline = if class == AdmitClass::Agent { a.inline } else { 0 };
                if class == AdmitClass::Agent {
                    ad.agent_bytes += len;
                    ad.agent_inline += agent_inline;
                }
                if let Some(actor) = a.actor {
                    *ad.pending_by.entry(actor).or_insert(0) += 1;
                    ad.seq_actor.insert(seq, actor);
                }
                let q = Queued { seq, id, cmd, bytes: len, agent_inline, at: now };
                ad.lanes[class.lane()].push(a.actor, q);
                seq
            }
        };
        let ticket = self.ticket(seq);
        self.pump();
        Ok(Admitted { ticket, class: admitted })
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
        if let Some(r) = lead.done.get(&ticket.seq) {
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

    /// One entry in flight: freezes the next command set once the previous entry committed.
    fn pump(&mut self) {
        loop {
            let no_lead = self.no_lead();
            let Some(lead) = self.lead.as_mut() else { return };
            // A transferring leader stops appending, and one with `no_lead_until` is handing off.
            if !lead.ready
                || lead.pending.is_some()
                || lead.deciding.is_some()
                || (lead.admission.is_empty() && lead.blocks.is_empty() && lead.records.is_empty())
                || lead.transfer.is_some()
                || no_lead
            {
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
            let batch = Self::cut(lead, self.now, self.admit_capacity);
            if batch.is_empty() {
                // Only agent commands wait, and the bucket is empty: `tick` cuts again once it refills.
                return;
            }
            // A command whose id has a row is not decided again: answer it from the stored events.
            let proof = lead.proof.clone().expect("ready implies a commit proof");
            let mut fresh = Vec::new();
            for q in batch {
                match self.fold.request(q.id) {
                    Some(row) => {
                        let acked = outcome::replay(row.index, &proof, q.cmd.replay(&row.events));
                        self.resolve(q.seq, Some(q.id), Ok(acked));
                    }
                    None => fresh.push(q),
                }
            }
            if fresh.is_empty() {
                continue;
            }
            let mut chains: SmallVec<[ChainId; 3]> = SmallVec::new();
            for q in &fresh {
                for c in q.cmd.deps(&self.fold.fold) {
                    if !chains.contains(&c) {
                        chains.push(c);
                    }
                }
            }
            if chains.is_empty() {
                self.decide(fresh, &[]);
            } else {
                let lead = self.lead.as_mut().expect("leading");
                lead.next_deps += 1;
                let id = lead.next_deps;
                lead.deciding = Some((id, fresh));
                self.out.deps.push(DepsWanted { id, chains });
            }
            return;
        }
    }

    /// The next entry's commands, by admission: Human and MergeQueue, then Agent up to AGENT_FLOOR of capacity over
    /// the time since the last cut, then Bot, then Agent while the AGENT_SHARE bucket holds a token. All within
    /// ENTRY_MAX bytes; the first command of an entry always goes. An entry's Agent inline pack bytes stay within
    /// AGENT_INLINE_ENTRY because admission stages any agent pack that would take the queued ones past it.
    fn cut(lead: &mut Lead<C>, now: Timestamp, fixed: Option<f32>) -> Vec<Queued<C>> {
        let ad = &mut lead.admission;
        let cap = ad.capacity(fixed);
        ad.refill(now, cap);
        let mut batch: Vec<Queued<C>> = Vec::new();
        let mut bytes = 0;
        let fits = |q: Option<&Queued<C>>, batch: &[Queued<C>], bytes: usize| {
            q.is_some_and(|q| batch.is_empty() || bytes + q.bytes <= ENTRY_MAX)
        };
        for class in [AdmitClass::Human, AdmitClass::MergeQueue] {
            while fits(ad.lanes[class.lane()].front(), &batch, bytes) {
                let q = ad.take(class).expect("front");
                bytes += q.bytes;
                batch.push(q);
            }
        }
        let since = (now.0.saturating_sub(ad.cut_at.0) as f32).max(ad.round_ms);
        let floor = (AGENT_FLOOR * cap * since / 1000.0).floor() as usize;
        let mut n = 0;
        while n < floor && ad.tokens >= 1.0 && fits(ad.agent().front(), &batch, bytes) {
            let q = ad.take(AdmitClass::Agent).expect("front");
            (bytes, n, ad.tokens) = (bytes + q.bytes, n + 1, ad.tokens - 1.0);
            batch.push(q);
        }
        while fits(ad.lanes[AdmitClass::Bot.lane()].front(), &batch, bytes) {
            let q = ad.take(AdmitClass::Bot).expect("front");
            bytes += q.bytes;
            batch.push(q);
        }
        while ad.tokens >= 1.0 && fits(ad.agent().front(), &batch, bytes) {
            let q = ad.take(AdmitClass::Agent).expect("front");
            (bytes, ad.tokens) = (bytes + q.bytes, ad.tokens - 1.0);
            batch.push(q);
        }
        if !batch.is_empty() {
            ad.cut_at = now;
        }
        batch
    }

    /// AGENT_WAIT: an agent command still queued that long is answered `Full` and never enters an entry.
    fn expire_agents(&mut self) {
        let Some(lead) = self.lead.as_mut() else { return };
        let cut = Timestamp(self.now.0.saturating_sub(AGENT_WAIT.as_millis() as u64));
        let ad = &mut lead.admission;
        let old = ad.lanes[AdmitClass::Agent.lane()].take_old(|q| q.at < cut);
        if old.is_empty() {
            return;
        }
        for q in &old {
            ad.agent_bytes -= q.bytes;
            ad.agent_inline -= q.agent_inline;
        }
        let retry_after_s = retry_after_s(ad.agent().len(), ad.capacity(self.admit_capacity));
        let now = self.now;
        for q in &old {
            if let Some(a) = ad.seq_actor.get(&q.seq).copied() {
                ad.refused(Some(a), now);
            }
        }
        for q in old {
            self.resolve(q.seq, Some(q.id), Err(CommitError::Full { retry_after_s }));
        }
    }

    /// The dependency folds a `DepsWanted` asked for, each read once through its chain's `fresh`.
    pub fn deps_read(&mut self, now: Timestamp, id: u64, deps: Vec<(ChainId, C::Dep)>) -> Ready<C, ()> {
        self.begin(now);
        let take = matches!(&self.lead, Some(l) if l.deciding.as_ref().is_some_and(|(d, _)| *d == id));
        if take {
            let (_, batch) = self.lead.as_mut().expect("leading").deciding.take().expect("deciding");
            self.decide(batch, &deps);
        }
        self.finish(())
    }

    /// Each command's `decide` sees the fold at `applied` plus the earlier commands of the same entry. The entry is
    /// appended even if every command was refused.
    fn decide(&mut self, batch: Vec<Queued<C>>, deps: &[(ChainId, C::Dep)]) {
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
            for q in &batch {
                tiers = tiers.union(q.cmd.tiers());
                format = format.max(q.cmd.format());
                match q.cmd.decide(&s, deps) {
                    Ok((evs, out)) => {
                        let start = events.len() as u32;
                        for e in &evs {
                            s.apply(e);
                        }
                        events.extend(evs);
                        requests.push((q.id, start..events.len() as u32));
                        decided.push((q.seq, Ok(out)));
                    }
                    Err(err) => {
                        self.out.refused.push((self.last().next(), q.id));
                        decided.push((q.seq, Err(err)));
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

    fn resolve(&mut self, seq: u64, id: Option<RequestId>, result: Outcome<C>) {
        let ticket = self.ticket(seq);
        let lead = self.lead.as_mut().expect("leading");
        lead.admission.resolved(seq);
        match id {
            Some(id) => {
                lead.by_request.remove(&id);
            }
            None => lead.by_request.retain(|_, s| *s != seq),
        }
        lead.done.insert(seq, result.clone());
        while lead.done.len() > DONE_KEEP {
            lead.done.pop_first();
        }
        self.out.resolutions.push(Resolution::Ticket { ticket, result });
    }

    /// The host reported the call for `round` to `to` failed. If it is the outstanding call, retry at once, at most
    /// COMMIT_RETRIES times in a row.
    pub(super) fn append_failed(&mut self, to: SiteId, round: u64) {
        let Some(lead) = self.lead.as_mut() else { return };
        if lead.sent.get(&to).is_none_or(|s| s.0 != round) {
            return;
        }
        lead.sent.remove(&to);
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

    fn send_append(&mut self, peer: SiteId, force: bool) {
        let (term, me, boot, now, commit) = (self.state.term, self.cfg.me, self.boot, self.now, self.state.commit);
        let last = self.last();
        let Some(lead) = self.lead.as_mut() else { return };
        if !force && lead.sent.contains_key(&peer) {
            return;
        }
        let next = *lead.next.entry(peer).or_insert(last.next());
        lead.matched.entry(peer).or_insert(Index(0));
        let transfer = lead.transfer.as_ref().map(|t| (t.to, t.id));
        lead.round += 1;
        let round = lead.round;
        lead.sent.insert(peer, (round, now));
        let prev = Index(next.0 - 1);
        if self.send_install(peer, prev, round) {
            return;
        }
        if prev < self.base.index {
            // Below the in-memory window: the host reads the entries from storage and calls `catchup`.
            let through = Index(self.base.index.0.min(prev.0 + CATCHUP_MAX as u64));
            self.out.catchup.push(super::Catchup { to: peer, round, from: prev, through });
            return;
        }
        // Subtract the absolute indexes in u64 first: on wasm32 a cast to usize would truncate them. The differences
        // are bounded by the in-memory window, so they fit.
        let from = (prev.0 - self.base.index.0) as usize;
        let to = ((last.0 - self.base.index.0) as usize).min(from + CATCHUP_MAX);
        let window = &self.log[from..to];
        let entries = window[..append_batch(window)].to_vec();
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
        self.send(peer, Message::Append(req));
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
            self.send(c.to, Message::Append(req));
        }
        self.finish(())
    }

    pub(super) fn on_append_res(&mut self, from: SiteId, res: AppendRes) {
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
        if res.round < lead.min_round {
            // Sent before the leader truncated its uncommitted tail: `matched` may name entries it no longer holds.
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
            self.hand_off();
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
            seqs.extend(decided.iter().map(|(s, _)| *s));
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
        lead.sent.clear();
        lead.ready_at = lead.ready_at.min(commit);
        for seq in seqs {
            self.resolve(seq, None, Err(CommitError::NotLeader(nl.clone())));
        }
        if self.last() > commit {
            self.truncate_from(commit.next());
        }
        self.broadcast(true);
    }

    /// A leader that must not lead (`no_lead_until`, or its site drained) transfers to a follower that may lead: the
    /// refuser when it qualifies, else the most caught up.
    pub(super) fn hand_off(&mut self) {
        let Some(lead) = &self.lead else { return };
        if lead.transfer.is_some() {
            return;
        }
        let mut to: Vec<SiteId> =
            self.peers().into_iter().filter(|s| self.may_lead_target(*s) && !self.quarantined(*s)).collect();
        let refuser = lead.refuser;
        to.sort_by_key(|s| (Some(*s) != refuser, std::cmp::Reverse(lead.matched(*s))));
        if let Some(t) = to.first().copied() {
            let _ = self.start_transfer(t, false);
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
                self.set_commit(n);
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
                    for (seq, result) in decided {
                        self.resolve(seq, None, outcome::settle(index, &proof, result));
                    }
                }
                self.after_ack();
                self.pump();
                return;
            }
            n = Index(n.0 - 1);
        }
    }

    /// The evidence for a commit at `n` by `acked`: each site's durable index as this leader counted it and when it
    /// learned it, with the entry's `at` and the time now.
    fn proof_for(&self, n: Index, acked: &BTreeSet<SiteId>) -> QuorumProof {
        let lead = self.lead.as_ref().expect("leading");
        let acks: Vec<DurableAck> = acked
            .iter()
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
            && now < Timestamp(lead.lease_end.0.saturating_sub(DRIFT.as_millis() as u64));
        lead.reads.push((id, now, !lease));
        if !lease {
            self.broadcast(true);
        }
        self.check_reads();
        Ok(id)
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
    /// the log already carries one, so one stall moves a lease once.
    fn take_extend(&self) -> Option<ExtendLeases> {
        let marker = self.state.stalled_since?;
        let applied = self.fold.applied();
        if self.log.iter().any(|e| e.index > applied && e.extend.is_some()) {
            return None;
        }
        let last = self.log.last().map_or(Timestamp(0), |e| e.at);
        let since = marker.max(last);
        let by = std::time::Duration::from_millis(self.now.0.saturating_sub(since.0));
        (!by.is_zero()).then_some(ExtendLeases { since, by })
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
        let last = self.last();
        // A transfer keeps sending rounds that carry its id, so the other follower accepts one.
        let waiting = !lead.ready || !lead.reads.is_empty() || lead.transfer.is_some();
        let due: Vec<SiteId> = self
            .targets()
            .into_iter()
            .filter(|p| {
                let gap = if self.state.cannot_follow.contains(p) { CANNOT_FOLLOW_RETRY } else { CALL_DEADLINE };
                let behind = lead.next.get(p).is_none_or(|n| *n <= last);
                let idle = lead.sent.get(p).is_none_or(|(_, at)| now >= after(*at, gap));
                idle && (behind || waiting)
            })
            .collect();
        for p in due {
            self.send_append(p, true);
        }
        self.check_reads();
        self.expire_agents();
        self.pump();
    }
}
