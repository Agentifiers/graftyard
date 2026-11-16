//! One group member as a sans-IO state machine.
//!
//! Every input takes the caller's clock and returns a `Ready`. `Ready::persist` hands the storage writes to a
//! `Storage` and only then releases the call's reply and its `Out` (messages, ticket resolutions, read grants,
//! dependency reads, alarms). A host that implements `Storage` with SQLite writes inside a Durable Object gets the
//! output gate's rule: no reply leaves before the writes it depends on are durable.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_types::{ChainId, IdBlock, PlacementError, PlacementRecord, Region, SiteId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::encode::{bootstrap, digest};
use crate::fold::{Command, Ev, Fold, GroupFold};
use crate::outcome::Acked;
use crate::read::ReadGrant;
use crate::types::{
    Colo, CommitError, Digest, Format, GroupConfig, Index, LogEntry, MemberState, NotLeader, ReleaseTag, ReplyMeta,
    RequestId, Term, Ticket, TransferId,
};
use crate::{BOOT_IDLE, after};

/// A window no log reaches: the default, under which `log()` holds every entry from index 1.
const KEEP_ALL: u64 = u64::MAX / 4;

mod election;
mod follower;
mod health;
mod install;
mod leader;
mod log;
mod transfer;

pub use install::{LEARNER_READ_EVERY, READ_PIPELINE};
pub use leader::{
    APPEND_BYTES_MAX, DONE_BYTES, LEARNERS_OUT_MAX, OUT_KEEP, OUT_MAX, OUTCOME_KEEP, PEER_OUT_MAX, PIPELINE,
    READ_ROUND_GAP,
};
pub use log::{FoldView, verify};

/// Who this member is and what it was built from. The host reads `record` from the record's source itself (the
/// `Names` leader, or genesis), so the member can check the bootstrap entry it is sent against its own.
#[derive(Clone, Debug)]
pub struct MemberConfig {
    pub chain: ChainId,
    pub me: SiteId,
    pub record: PlacementRecord,
    /// The region of every site the group may name, for `QuorumProof.regions`.
    pub regions: Vec<(SiteId, Region)>,
    pub release: ReleaseTag,
    /// The command and event formats this release decodes. `RAFT_FORMAT` is always read.
    pub reads: Vec<Format>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Message<E> {
    Append(AppendReq<E>),
    AppendRes(AppendRes),
    Vote(VoteReq),
    VoteRes(VoteRes),
    /// A member with no committed config proposes the bootstrap entry. A receiver stores it only if its bytes equal
    /// the entry it builds from its own record.
    Propose(LogEntry<E>),
    Proposed {
        stored: bool,
    },
    /// A leader's transfer target, caught up: campaign now for `term + 1` with the transfer id in the vote request.
    TimeoutNow {
        term: Term,
        transfer: TransferId,
    },
    /// The target will not campaign for this transfer (it may not lead, or it is quarantined), so the leader abandons
    /// at once instead of retrying until the LEASE runs out.
    TransferRefused {
        term: Term,
        transfer: TransferId,
    },
    /// A leader whose commit-error rule tripped asks the other follower to ping `target` (`linkErrors`).
    Probe {
        term: Term,
        id: u64,
        target: SiteId,
    },
    /// `failed` of `of` pings to `target` failed or missed CALL_DEADLINE.
    ProbeRes {
        term: Term,
        id: u64,
        target: SiteId,
        failed: u8,
        of: u8,
    },
    Ping {
        probe: u64,
        n: u8,
    },
    Pong {
        probe: u64,
        n: u8,
    },
    /// Leader to a member whose next entry it has archived (`design/DESIGN.md`, Shape, "Packs").
    Install(InstallSnapshot),
    /// A follower or learner asks the leader for a read index for the reads in batch `batch`.
    ReadIndex {
        batch: u64,
    },
    /// The leader's answer: the applied index a read there sees and that entry's digest, or `None` when it cannot
    /// serve one. The index is committed, so an asker whose entry there has the same digest commits through it.
    ReadIndexRes {
        batch: u64,
        index: Option<(Index, Digest)>,
    },
}

/// The design's `InstallSnapshot`, with the keys as the strings the host names objects by (`crates/raft` cannot name
/// `R2Key`), and four additions: the round fields an AppendEntries carries, so the answer is an `AppendRes` to that
/// round, and the term and digest of entry `through`, so the member checks what it loaded and the next entry chains.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSnapshot {
    pub term: Term,
    pub boot: u64,
    pub round: u64,
    pub sent_at: Timestamp,
    pub lead_seq: u64,
    pub source: InstallSource,
}

/// What a leader names in an `InstallSnapshot`, which its host sets with `Member::set_install` once its storage no
/// longer holds the entries at or below `through`: the newest committed snapshot of each tier, the archive objects
/// through `through`, entry `through`'s term and digest, and the group fold's two parts no tier snapshot holds, the
/// committed config and id block as of `through`. The archives start after the snapshot or `REQUEST_RETENTION` before
/// `through`, whichever is earlier, because the installing host rebuilds the `request` rows from them: a member
/// without them would decide a re-sent request again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSource {
    pub core: Box<str>,
    pub metadata: Option<Box<str>>,
    pub archives: SmallVec<[Box<str>; 8]>,
    pub through: Index,
    pub through_term: Term,
    pub through_digest: Digest,
    pub config: (Index, GroupConfig),
    pub id_block: Option<IdBlock>,
}

/// An `InstallSnapshot` this member needs: the host loads the snapshot and archives it names and calls
/// `Member::install`. A host runs one at a time and drops repeats, which the leader sends every CALL_DEADLINE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallWanted {
    pub from: SiteId,
    pub install: InstallSnapshot,
}

/// Why `Member::install` refused what the host loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallError {
    /// The loaded fold is not at the install's `through`, or holds another config or id block.
    Mismatch { what: &'static str },
    /// This member has committed `through` already, or leads.
    NotNeeded,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope<E> {
    pub from: SiteId,
    pub to: SiteId,
    pub msg: Message<E>,
}

/// AppendEntries. `boot`, `round`, and `sent_at` name the leader instance and the round, and the reply echoes them,
/// so an ack extends the lease from the time its round was sent and a reset leader ignores its predecessor's acks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppendReq<E> {
    pub term: Term,
    pub leader: SiteId,
    pub boot: u64,
    pub round: u64,
    pub sent_at: Timestamp,
    pub prev_index: Index,
    pub prev_term: Term,
    pub prev_digest: Digest,
    pub entries: Vec<LogEntry<E>>,
    pub commit: Index,
    /// While a transfer runs: its target and id. Acks to these rounds extend no lease.
    pub transfer: Option<(SiteId, TransferId)>,
    /// The leader instance's `MemberState.lead_seq`: within one term, a follower refuses an older instance's append.
    pub lead_seq: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppendRes {
    pub term: Term,
    pub boot: u64,
    pub round: u64,
    pub sent_at: Timestamp,
    pub prev_index: Index,
    pub ok: bool,
    /// The last index this follower holds that matches the leader's log, when `ok`.
    pub matched: Index,
    pub last: Index,
    /// The follower promised: no vote for a rival until `ELECTION_AFTER` from its receipt.
    pub promise: bool,
    /// `verify` refused an entry. Each refusal carries the refuser's release.
    pub refused: Option<Refusal>,
    pub stepped_down_at: Option<Timestamp>,
    /// The follower's last two boots, so a leader picks no transfer target inside RESET_QUARANTINE.
    pub boots: [Timestamp; 2],
    pub release: ReleaseTag,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoteReq {
    pub term: Term,
    pub candidate: SiteId,
    pub pre: bool,
    pub last_index: Index,
    pub last_term: Term,
    pub transfer: Option<TransferId>,
    pub boots: [Timestamp; 2],
    pub stepped_down_at: Option<Timestamp>,
    /// The candidate holds the bootstrap entry as committed. A voter with no committed config that holds, or can
    /// build, the same entry learns the commit from this, because every voter builds the same bytes for index 1.
    pub bootstrapped: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoteRes {
    /// The voter's term.
    pub term: Term,
    /// The term the request asked for.
    pub asked: Term,
    pub pre: bool,
    pub granted: bool,
    pub deny: Option<Deny>,
}

/// Why a voter refused a vote or a pre-vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Deny {
    Stale,
    /// The voter has no committed config, so it grants no vote.
    NoConfig,
    /// The candidate is not a voter of the voter's latest committed config.
    NotMember,
    Leader,
    /// The voter promised another leader and the promise has not lapsed.
    Promised,
    LogBehind,
    /// Both campaign at once; the lower site wins the pre-vote.
    Contending,
    /// Two boots, or a `STEP_DOWN`, inside `RESET_QUARANTINE`.
    Quarantine,
    Voted,
    /// The candidate's site is drained, or under the first day's lead limit.
    Drained,
    /// Two colos reported the candidate as this voter's leader within the SUSPECT window, less than 2 x that window
    /// ago. The candidate stands aside for 2 x the window.
    Suspected,
}

/// A follower's `verify` refusal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub release: ReleaseTag,
    pub index: Index,
    pub reason: Box<str>,
}

/// One storage write. A host applies them in order, and they become durable together.
#[derive(Clone, Debug, PartialEq)]
pub enum Write<E> {
    /// The member's `MemberState` row.
    State(MemberState),
    /// Store the entry at its index; every lower index is stored.
    Append(LogEntry<E>),
    /// Delete every entry at or above the index. Never at or below `commit`.
    Truncate(Index),
    /// Apply stored entries through the index to the fold, the `request` table, and the committed config.
    Apply(Index),
    /// The member installed a snapshot (`Member::install`): its log is empty above this base, and its fold, request
    /// rows, config, and id block are the ones the host passed in, which the host stores with this write.
    Install(LogBase),
}

/// Where a host makes writes durable. `Ready::persist` is the only way to the reply and the outputs.
pub trait Storage<E> {
    fn write(&mut self, w: &Write<E>);
}

/// The committed entry just below a member's in-memory log window: its index, term, and digest. The entries at and
/// below it are on the host's storage only, and the leader asks for them with `Out::catchup` when a follower needs
/// them. Index 0 (term 0, `PREV_ZERO`) before anything was compacted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogBase {
    pub index: Index,
    pub term: Term,
    pub digest: Digest,
}

impl Default for LogBase {
    fn default() -> Self {
        LogBase { index: Index(0), term: Term(0), digest: crate::PREV_ZERO }
    }
}

/// A follower behind the leader's window: send it the stored entries `from..=through` with `Member::catchup`, for
/// AppendEntries round `round`. The entries are committed, so any later answer is still right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Catchup {
    pub to: SiteId,
    pub round: u64,
    pub from: Index,
    pub through: Index,
}

/// The reference storage: what a member's SQLite holds. Tests use it as the disk that survives a crash. `log` holds
/// the entries above `base`; a host that keeps the whole log loads only the window above the base it picks.
#[derive(Clone, Debug)]
pub struct Stored<F: Fold> {
    pub state: Option<MemberState>,
    pub base: LogBase,
    pub log: Vec<LogEntry<F::Event>>,
    pub fold: GroupFold<F>,
}

impl<F: Fold> Stored<F> {
    pub fn new(fold: F) -> Stored<F> {
        Stored { state: None, base: LogBase::default(), log: Vec::new(), fold: GroupFold::new(fold) }
    }
}

impl<F: Fold> Storage<F::Event> for Stored<F> {
    fn write(&mut self, w: &Write<F::Event>) {
        match w {
            Write::State(s) => self.state = Some(s.clone()),
            Write::Append(e) => {
                assert_eq!(e.index.0, self.base.index.0 + self.log.len() as u64 + 1, "append with a gap");
                self.log.push(e.clone());
            }
            Write::Truncate(i) => {
                assert!(*i > self.fold.applied(), "truncate of an applied entry");
                self.log.truncate((i.0 - self.base.index.0) as usize - 1);
            }
            // The installed fold is the host's to store beside this write (the fold `Member::install` took).
            Write::Install(b) => {
                self.base = *b;
                self.log.clear();
            }
            Write::Apply(i) => {
                while self.fold.applied() < *i {
                    let e = &self.log[(self.fold.applied().0 - self.base.index.0) as usize];
                    self.fold.apply(e);
                }
            }
        }
    }
}

/// What the leader's admission sees (`Member::admission`): the hints the edge reads to stage packs, and the queue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdmissionView {
    /// Commands a second, measured or fixed.
    pub capacity: f32,
    pub agent_queued: usize,
    pub agent_bytes: usize,
    /// Every class.
    pub queued: usize,
    pub hot: bool,
    pub pressure: bool,
    pub retry_after_s: u32,
    /// The moving averages `capacity` is sized from: the commit round in ms and a command's bytes as ENTRY_SOFT
    /// counts them. Read only by test hosts tracing admission (LM-1's B0-31).
    pub round_ms: f32,
    pub cmd_bytes: f32,
}

/// A read the host started with `Member::read`; its `Resolution::Read` carries the same id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadId(pub u64);

/// The answer to `Member::read`.
pub type ReadStart = Result<ReadId, NotLeader>;

/// A command's dependency chains, which the host reads once through each chain's `fresh` and passes to
/// `Member::deps_read` with the same `id`. The entry is decided only then.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepsWanted {
    pub id: u64,
    pub chains: SmallVec<[ChainId; 3]>,
}

/// The entry at `index` was decided on the reads of these `DepsWanted` ids. A batch's reads may start while the
/// previous entry is in flight, and an id block or recorded entry may go first, so only this names the entry a read
/// decided. Hosts may ignore it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DepsUsed {
    pub ids: SmallVec<[u64; 2]>,
    pub index: Index,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Alarm {
    /// A follower on the same or a newer release refused an entry, or any follower refused a committed one. The
    /// leader keeps leading and adds it to `cannot_follow`; `graft-ops` halts the site day.
    Refused { site: SiteId, refusal: Refusal },
    /// A follower on an older release refused an uncommitted entry: the leader truncated its uncommitted tail,
    /// persisted `no_lead_until`, and transfers.
    NoLead { site: SiteId, refusal: Refusal },
    /// Both followers on the leader's own release refused one uncommitted entry: its own defect. It truncated the
    /// uncommitted tail and keeps leading.
    OwnDefect { index: Index },
    /// `STEP_DOWN` tripped. `to` is the transfer target, or `None` when the leader holds for one window.
    StepDown { why: StepDownWhy, to: Option<SiteId> },
}

/// What tripped `STEP_DOWN`, with the window's numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepDownWhy {
    /// Per follower: (site, counted calls, failed calls), after the own-stall exclusion.
    CommitErrors { calls: Vec<(SiteId, u32, u32)> },
    /// The window's p99 and how many of `of` confirms passed `confirm_p99_ms`.
    Confirm { p99_ms: u32, slow: u32, of: u32 },
    /// The event loop is behind (LOOP_LAG): `clamped` confirms in LOOP_LAG's window read whole 100 ms ticks.
    LoopLag { clamped: u32 },
}

/// A member-to-member call the host saw fail: an error, or no reply within CALL_DEADLINE. `Envelope::failure` builds
/// it from the request the member sent. Only AppendEntries and probe pings are counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallFailure {
    pub to: SiteId,
    pub call: FailedCall,
    /// The call missed CALL_DEADLINE, rather than failing with an error. Only a deadline miss can be excused by the
    /// member's own outstanding storage confirm.
    pub deadline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailedCall {
    Append { term: Term, boot: u64, round: u64, sent_at: Timestamp },
    Ping { probe: u64, n: u8 },
}

impl<E> Envelope<E> {
    /// The failure report for this request, if it is a call the member counts.
    pub fn failure(&self, deadline: bool) -> Option<CallFailure> {
        let call = match &self.msg {
            Message::Append(a) => FailedCall::Append { term: a.term, boot: a.boot, round: a.round, sent_at: a.sent_at },
            Message::Ping { probe, n } => FailedCall::Ping { probe: *probe, n: *n },
            _ => return None,
        };
        Some(CallFailure { to: self.to, call, deadline })
    }
}

/// One storage confirm the host reported begun with `Member::confirm_begin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConfirmId(pub u64);

/// The answer to `Member::campaign` (the `suspect` path), and the end of a campaign it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Leader,
    /// A restarted leader resumed `led_term`; it serves once one round reaches a quorum.
    Resumed,
    /// Pre-vote sent.
    Started,
    /// No committed config: this member proposed the bootstrap entry.
    Bootstrapping,
    Promised,
    Busy,
    Won,
    Lost,
    /// This member's site is drained or `no_lead_until` holds, so it does not campaign (until it has seen no leader
    /// for 2 x ELECTION_AFTER).
    MayNotLead,
}

pub enum Resolution<C: Command> {
    /// A ticket resolved: on the commit-advance path once its entry committed, or `NotLeader` when the leadership
    /// that held it ended.
    Ticket {
        ticket: Ticket,
        result: Result<Acked<C::Out>, CommitError<C::Err>>,
    },
    Read {
        id: ReadId,
        result: Result<ReadGrant, NotLeader>,
    },
    Campaign(Verdict),
    /// An id block `Member::id_block` took: committed at the index, or `NotLeader` when the leadership that held it
    /// ended. A block lost with its leader may still commit under the next one; the host asks again.
    IdBlock {
        block: IdBlock,
        result: Result<Index, NotLeader>,
    },
    /// Events `Member::record` took under request id `id`: committed at the index (an earlier commit's, when the id
    /// had a row already), or `NotLeader` when the leadership that held them ended. Events lost with their leader may
    /// still commit under the next one; recording them again under the same id cannot apply them twice.
    Recorded {
        id: RequestId,
        result: Result<Index, NotLeader>,
    },
}

/// Why `Member::reconfigure` did not append a config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconfigError {
    NotLeader(NotLeader),
    /// The leader is not ready, has an entry or a transfer in flight, or its latest config has not committed.
    Busy,
    /// The change adds or removes more than one voter site, or changes the member on a site it keeps.
    NotOneChange,
    /// The new config does not name the leader's site: transfer first.
    RemovesLeader,
}

/// Why `Member::restore` refused what the host read back.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    #[error(transparent)]
    Placement(#[from] PlacementError),
    /// The stored fold is not inside the stored log window: `base <= applied <= last`, where `last` is
    /// `base + log.len()`.
    #[error("stored fold at {applied:?} is outside the log window {base:?}..={last:?}")]
    Window { base: Index, applied: Index, last: Index },
}

/// Why `Member::id_block` did not take a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdBlockError {
    NotLeader(NotLeader),
    /// The block does not start at or after the end of the group's latest one (`current`): it was recorded already,
    /// or is older.
    Behind {
        current: IdBlock,
    },
}

impl<C: Command> std::fmt::Debug for Resolution<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Resolution::Ticket { ticket, result } => write!(f, "Ticket({ticket:?}, {result:?})"),
            Resolution::Read { id, result } => write!(f, "Read({id:?}, {result:?})"),
            Resolution::Campaign(v) => write!(f, "Campaign({v:?})"),
            Resolution::IdBlock { block, result } => write!(f, "IdBlock({block:?}, {result:?})"),
            Resolution::Recorded { id, result } => write!(f, "Recorded({id:?}, {result:?})"),
        }
    }
}

/// What `outcome_by_request` finds for a request id.
pub enum ByRequest<C: Command> {
    /// The request's entry committed; the output is replayed from its stored events.
    Done(Acked<C::Out>),
    Pending(Ticket),
    Unknown,
}

/// What a call released once its writes were handed to storage.
///
/// Everything but `early` may leave only once those writes are durable. `early` is the leader's AppendEntries, which
/// may leave before them, and should, so the leader's own write overlaps the round (Raft dissertation 10.2.1). The
/// leader counts itself toward a commit of its new entries only after `self_ack` comes back through
/// `Member::self_ack`, which the host calls once the writes are durable.
pub struct Out<C: Command> {
    pub early: Vec<Envelope<Ev<C>>>,
    pub messages: Vec<Envelope<Ev<C>>>,
    pub resolutions: Vec<Resolution<C>>,
    pub deps: Vec<DepsWanted>,
    pub deps_used: Vec<DepsUsed>,
    pub alarms: Vec<Alarm>,
    /// Per entry this input decided, the admitted requests whose command `decide` refused, each with the core's own
    /// copy of the command, moved out of the batch. Each refusal's ticket resolves `Refused` only if that entry
    /// commits; if it is lost, the re-sent command is decided again. A host that re-sends a refusal itself (the
    /// `NeedIdBlock` lease loop) takes the command from here instead of keeping a copy of every admitted command.
    pub refused: Vec<(Index, RequestId, C)>,
    /// Present when these writes append to this leader's log.
    pub self_ack: Option<SelfAck>,
    /// Followers behind this leader's in-memory window: read the entries from storage and pass them to
    /// `Member::catchup`. Not held for the writes, since the entries are committed.
    pub catchup: Vec<Catchup>,
    /// Snapshots this member must install before it can follow (`Member::install`).
    pub install: Vec<InstallWanted>,
}

impl<C: Command> Default for Out<C> {
    fn default() -> Self {
        Out {
            early: Vec::new(),
            messages: Vec::new(),
            resolutions: Vec::new(),
            deps: Vec::new(),
            deps_used: Vec::new(),
            alarms: Vec::new(),
            refused: Vec::new(),
            self_ack: None,
            catchup: Vec::new(),
            install: Vec::new(),
        }
    }
}

/// The leader's own entries through `index` are in the writes of the `Ready` that released this. Only `persist`
/// hands one out, and it is neither `Clone` nor constructible outside the crate, so the only way for a leader to
/// count its own copy is to give this back to `Member::self_ack` after those writes are durable. A host that drops
/// it loses liveness, not safety: the leader then never commits those entries. One from another boot, or from
/// before a truncation of the log, is ignored.
///
/// The host confirms writes in order, as a Durable Object does: once one `Ready`'s writes are durable, so are those
/// of every earlier `Ready`.
#[derive(Debug, PartialEq, Eq)]
pub struct SelfAck {
    boot: u64,
    epoch: u64,
    index: Index,
}

/// The result of one input: writes that must be durable, and what may leave once they are.
#[must_use = "persist the writes, then send what `persist` returns"]
pub struct Ready<C: Command, R> {
    pub writes: Vec<Write<Ev<C>>>,
    reply: R,
    out: Out<C>,
}

impl<C: Command, R> Ready<C, R> {
    /// Hands every write to `store`, in order, and only then returns the reply and the outputs. The host may send
    /// `Out::early` as soon as it has them; everything else waits until the writes are durable, and `Out::self_ack`
    /// goes back to `Member::self_ack` then.
    pub fn persist<S: Storage<Ev<C>>>(self, store: &mut S) -> (R, Out<C>) {
        for w in &self.writes {
            store.write(w);
        }
        (self.reply, self.out)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

pub struct Member<C: Command> {
    cfg: MemberConfig,
    boot_entry: LogEntry<Ev<C>>,
    boot_digest: Digest,
    state: MemberState,
    /// The committed entry below `log`, which holds the entries above it: at most `2 x window` applied ones, plus
    /// any not yet applied. Index 0 unless the host set a window.
    base: LogBase,
    window: u64,
    /// At most this many bytes of applied entries stay in memory (`set_log_window`), beside `window`.
    window_bytes: usize,
    /// `applied` when the current input began, so `compact` keeps what this input applied.
    input_applied: Index,
    log: Vec<LogEntry<Ev<C>>>,
    digests: Vec<Digest>,
    /// Each entry's canonical length, beside `digests`, and their sum.
    sizes: Vec<usize>,
    log_bytes: usize,
    /// Indexes of the `Config` entries in the log, oldest first.
    configs: Vec<Index>,
    fold: GroupFold<C::Fold>,
    role: Role,
    leader: Option<SiteId>,
    last_leader_seen: Timestamp,
    /// The last AppendEntries this member accepted from a leader, withheld or not.
    appended_at: Timestamp,
    campaign: Option<election::Campaign>,
    /// The voters that stored the bootstrap entry this member proposed, while it waits for a quorum.
    /// And when it started and last sent the proposal.
    boot_acks: Option<(BTreeSet<SiteId>, Timestamp, Timestamp)>,
    backoff_until: Timestamp,
    lead: Option<leader::Lead<C>>,
    /// Drawn from the seed when the object starts. Tickets and rounds carry it.
    boot: u64,
    rng: u64,
    /// Counts inputs, so a `ReadGrant` is used before the next one.
    step: u64,
    now: Timestamp,
    next_read: u64,
    writes: Vec<Write<Ev<C>>>,
    out: Out<C>,
    dirty: bool,
    /// `suspect`: the colos that reported this member's leader in `state.term`, and when.
    reports: BTreeMap<Colo, Timestamp>,
    /// No promise to any leader before this: two colos reported the leader (`suspect`). Ends on a higher term.
    withhold_until: Timestamp,
    /// Leaders two colos reported, and until when pre-vote refuses them. A higher term does not end it.
    suspected: BTreeMap<SiteId, Timestamp>,
    /// Leaders in `suspected` whose bar every report set while they ran a transfer (`on_suspect`). Accepting another
    /// leader's append ends it: the hand-off completed, so the reports measured it, not a gray leader. Kept, two leaders
    /// in turn, each reported during its own hand-off, refused each other as `Suspected` while a half-hung third won
    /// and lost again, and nobody led for 31.3 s (F67 rule 11, live seed 11121).
    handing_off: BTreeSet<SiteId>,
    /// The host's storage confirms: begin, and end once confirmed. Kept 30 s after they end.
    confirms: BTreeMap<ConfirmId, (Timestamp, Option<Timestamp>)>,
    next_confirm: u64,
    /// A `linkErrors` probe this member runs for its leader, and the last one's result: (id, target, failed, of).
    probing: Option<health::Prober>,
    probed: Option<(u64, SiteId, u8, u8)>,
    /// The release of each follower in `cannot_follow` when it refused, so it leaves the set when its release changes.
    refuser_release: BTreeMap<SiteId, ReleaseTag>,
    /// Sites whose members may not lead: drained, or under the first day's lead limit. The host sets it from the
    /// `Release` group's site modes.
    drained: BTreeSet<SiteId>,
    /// A voter refused this member's pre-vote as `Suspected`: until then it denies no rival as `Contending`.
    stand_aside_until: Timestamp,
    /// The leader this member forgot because its read-index call to it failed (F67): reports about it still count.
    /// A leader it never heard from (a transfer target it voted for) is not one: reports about it do not count.
    forgot: Option<SiteId>,
    /// A voter refused this member's pre-vote as `Suspected`: it starts no other campaign until then, half the bar.
    retry_pre_at: Timestamp,
    /// The term this member last granted a rival's pre-vote for, and until when it starts no campaign of its own:
    /// the rival asked first, so it gets one CALL_DEADLINE to win. Once per asked term.
    pre_granted: (Term, Timestamp),
    /// The highest index of this member's log that it knows is durable on its own storage: the stored log at
    /// `restore`, then raised only by `self_ack`. A leader counts itself toward a commit only up to here.
    own_durable: Index,
    /// When `own_durable` last rose, on this member's clock: `restore`, then each `self_ack` that raised it.
    own_durable_at: Timestamp,
    /// Counts truncations, so a `SelfAck` from before one does not vouch for an entry appended after it.
    epoch: u64,
    /// The last entry the current input appended as leader, for the input's `SelfAck`.
    own_appended: Option<Index>,
    /// AppendEntries go out early only once `own_durable` reaches this: a resumed `led_term` sends its `Noop`s only
    /// after they are durable (`leader::become_leader`).
    early_from: Index,
    /// What this member names, as leader, to a member behind its archive boundary (`set_install`).
    install: Option<InstallSource>,
    /// Reads waiting on a read index from the leader, as a follower or learner.
    remote: install::RemoteReads,
    /// A fixed admission capacity in commands a second (`set_admit_capacity`); measured when `None`.
    admit_capacity: Option<f32>,
    /// Where `cut` stops filling an entry past its first command (`set_entry_soft`).
    entry_soft: usize,
    /// The host's clock may lag real time by DRIFT / 2 or more (`set_clock_suspect`): a leader then grants no read on
    /// its lease, only after a round sent after the read arrived.
    clock_suspect: bool,
    /// The last `DepsWanted` id. Never reset by a new leadership, so a late answer to an earlier leadership's read
    /// cannot decide a later batch; it starts from the boot value, so neither can one from before a restart.
    next_deps: u64,
    /// AppendEntries bytes alive per call this member sent as leader, by (peer, round): from the send until its answer,
    /// an error, `call_ended`, or OUT_KEEP. Kept across leaderships, because the host still awaits an earlier one's
    /// calls (`leader::out_allows`).
    calls_out: BTreeMap<(SiteId, u64), (Timestamp, usize)>,
    /// The last round any leadership of this boot sent, so rounds never repeat and `calls_out` keys stay distinct.
    rounds: u64,
    /// The last ticket seq any leadership of this boot issued, so a leadership that resumes the same term in the same
    /// boot (after a `Noop` that reached no quorum) never reissues a ticket an edge may still ask `outcome` for.
    seqs: u64,
}

impl<C: Command> Member<C> {
    /// A member that has stored nothing yet.
    pub fn new(cfg: MemberConfig, seed: u64) -> Result<Member<C>, PlacementError>
    where
        C::Fold: Default,
    {
        Self::build(cfg, MemberState::fresh(), LogBase::default(), Vec::new(), GroupFold::new(C::Fold::default()), seed)
    }

    /// A member after its object restarted: everything from `stored`, nothing else. A boot within `BOOT_IDLE` of
    /// the last saved state interrupted work and goes into `boots`. It takes `stored` whole, so the log and fold a
    /// host read back are never held twice: a clone of them took a member's isolate past 128 MB (F47).
    pub fn restore(
        cfg: MemberConfig,
        stored: Stored<C::Fold>,
        now: Timestamp,
        seed: u64,
    ) -> Result<(Member<C>, Ready<C, ()>), RestoreError> {
        let Stored { state, base, log, fold } = stored;
        let last = Index(base.index.0 + log.len() as u64);
        if !(base.index <= fold.applied() && fold.applied() <= last) {
            return Err(RestoreError::Window { base: base.index, applied: fold.applied(), last });
        }
        let mut fold = fold;
        // The admission window is not in the host's tables: rebuild it from the applied entries the stored window
        // holds (`GroupFold::note_writes`; an entry already noted is skipped).
        let applied = fold.applied();
        for e in log.iter().filter(|e| e.index <= applied) {
            fold.note_writes(e);
        }
        let mut m = Self::build(cfg, state.unwrap_or_else(MemberState::fresh), base, log, fold, seed)?;
        m.begin(now);
        m.own_durable_at = now;
        if m.state.saved_at.0 != 0 && now < after(m.state.saved_at, BOOT_IDLE) {
            m.state.boots = [m.state.boots[1], now];
            m.dirty = true;
        }
        // `no_lead_until` clears when this member's release changes.
        if m.state.no_lead_until.is_some_and(|r| r != m.cfg.release.id) {
            m.state.no_lead_until = None;
            m.dirty = true;
        }
        let r = m.finish(());
        Ok((m, r))
    }

    fn build(
        cfg: MemberConfig,
        state: MemberState,
        base: LogBase,
        log: Vec<LogEntry<Ev<C>>>,
        fold: GroupFold<C::Fold>,
        seed: u64,
    ) -> Result<Member<C>, PlacementError> {
        let boot_entry = bootstrap(cfg.chain, &cfg.record)?;
        let boot_digest = digest(&boot_entry);
        let (digests, sizes): (Vec<Digest>, Vec<usize>) = log.iter().map(crate::encode::digest_len).unzip();
        let log_bytes = sizes.iter().sum();
        let configs = log.iter().filter(|e| matches!(e.body, crate::EntryBody::Config(_))).map(|e| e.index).collect();
        // The last time this member heard a leader, as far as its persisted row says: a promise is given on an
        // append, ELECTION_AFTER before it lapses. A restarted voter so keeps its quarantine waiver honest.
        let last_leader_seen =
            Timestamp(state.promise_until.0.saturating_sub(crate::ELECTION_AFTER.as_millis() as u64));
        let mut rng = seed ^ 0x9e37_79b9_7f4a_7c15;
        let boot = splitmix(&mut rng);
        // Everything `restore` read back is on storage.
        let own_durable = Index(base.index.0 + log.len() as u64);
        Ok(Member {
            cfg,
            boot_entry,
            boot_digest,
            state,
            base,
            window: KEEP_ALL,
            window_bytes: usize::MAX,
            input_applied: Index(0),
            log,
            digests,
            sizes,
            log_bytes,
            configs,
            fold,
            role: Role::Follower,
            leader: None,
            last_leader_seen,
            appended_at: Timestamp(0),
            campaign: None,
            boot_acks: None,
            backoff_until: Timestamp(0),
            lead: None,
            boot,
            rng,
            step: 0,
            now: Timestamp(0),
            next_read: 0,
            writes: Vec::new(),
            out: Out::default(),
            dirty: false,
            reports: BTreeMap::new(),
            withhold_until: Timestamp(0),
            suspected: BTreeMap::new(),
            handing_off: BTreeSet::new(),
            confirms: BTreeMap::new(),
            next_confirm: 0,
            probing: None,
            probed: None,
            refuser_release: BTreeMap::new(),
            drained: BTreeSet::new(),
            stand_aside_until: Timestamp(0),
            forgot: None,
            retry_pre_at: Timestamp(0),
            pre_granted: (Term(0), Timestamp(0)),
            own_durable,
            own_durable_at: Timestamp(0),
            epoch: 0,
            own_appended: None,
            early_from: Index(0),
            install: None,
            remote: install::RemoteReads::starting_at(boot),
            admit_capacity: None,
            entry_soft: crate::ENTRY_SOFT,
            clock_suspect: false,
            // The top bit clear leaves 2^63 ids before a wrap.
            next_deps: boot >> 1,
            calls_out: BTreeMap::new(),
            rounds: 0,
            seqs: 0,
        })
    }

    /// A message from another member.
    pub fn step(&mut self, now: Timestamp, from: SiteId, msg: Message<Ev<C>>) -> Ready<C, ()> {
        self.begin(now);
        match msg {
            Message::Append(req) => self.on_append(from, req),
            Message::AppendRes(res) => self.on_append_res(from, res),
            Message::Vote(req) => self.on_vote(from, req),
            Message::VoteRes(res) => self.on_vote_res(from, res),
            Message::Propose(e) => self.on_propose(from, e),
            Message::Proposed { stored } => self.on_proposed(from, stored),
            Message::TimeoutNow { term, transfer } => self.on_timeout_now(from, term, transfer),
            Message::TransferRefused { term, transfer } => self.on_transfer_refused(term, transfer),
            Message::Probe { term, id, target } => self.on_probe(from, term, id, target),
            Message::ProbeRes { term, id, target, failed, of } => self.on_probe_res(term, id, target, failed, of),
            Message::Ping { probe, n } => self.send(from, Message::Pong { probe, n }),
            Message::Pong { probe, n } => self.on_pong(from, probe, n, true),
            Message::Install(req) => self.on_install(from, req),
            Message::ReadIndex { batch } => self.on_read_index(from, batch),
            Message::ReadIndexRes { batch, index } => self.on_read_index_res(from, batch, index),
        }
        self.finish(())
    }

    /// Time passing: resends after CALL_DEADLINE, campaign, read, transfer, and probe deadlines, and the leader's
    /// health rules (`STEP_DOWN`, transfer back to `home`). Starts no campaign.
    pub fn tick(&mut self, now: Timestamp) -> Ready<C, ()> {
        self.begin(now);
        self.tick_election();
        // An abandoned transfer ends before this tick's resends, so none of them carries its id.
        self.tick_transfer();
        self.tick_leader();
        self.tick_probe();
        self.tick_health();
        self.tick_remote();
        self.finish(())
    }

    pub fn me(&self) -> SiteId {
        self.cfg.me
    }
    pub fn role(&self) -> Role {
        self.role
    }
    pub fn term(&self) -> Term {
        self.state.term
    }
    pub fn state(&self) -> &MemberState {
        &self.state
    }
    pub fn leader(&self) -> Option<SiteId> {
        self.leader
    }
    /// The entries in memory: those above `log_base()`.
    pub fn log(&self) -> &[LogEntry<Ev<C>>] {
        &self.log
    }
    /// `digests()[i]` is the digest of the entry at index `log_base().index + i + 1`.
    pub fn digests(&self) -> &[Digest] {
        &self.digests
    }
    pub fn log_base(&self) -> LogBase {
        self.base
    }
    /// The last index this member holds.
    pub fn last_index(&self) -> Index {
        self.last()
    }
    /// The entry at `i`, if it is in memory.
    pub fn entry_at(&self, i: Index) -> Option<&LogEntry<Ev<C>>> {
        (i > self.base.index && i <= self.last()).then(|| self.entry(i))
    }
    /// The digest of the entry at `i`, if it is in memory or is the base.
    pub fn digest_of(&self, i: Index) -> Option<Digest> {
        self.digest_at(i)
    }
    /// Keep `n` applied entries in memory, and drop the older ones once twice as many have applied; a host that does
    /// must serve `Out::catchup` from its storage. By default every entry stays in memory. Call it before the first
    /// input, with the same `n` after every restore: `restore` compacts at once if the stored window is larger.
    pub fn set_log_window(&mut self, n: u64) {
        self.set_log_window_bytes(n, usize::MAX);
    }

    /// `set_log_window`, and also keep at most `bytes` of applied entries (their canonical encoding) in memory,
    /// dropping the oldest applied ones once they pass it, though never the last applied entry. Entries not yet
    /// applied stay whatever their size. A host that loads a window at restore bounds it the same way
    /// (`workers/site`, `load`, with `LOG_BYTES`).
    pub fn set_log_window_bytes(&mut self, n: u64, bytes: usize) {
        self.window = n.clamp(1, KEEP_ALL);
        self.window_bytes = bytes;
        self.compact();
    }

    /// Bytes of the entries in memory, applied or not, by their canonical encoding.
    pub fn log_bytes(&self) -> usize {
        self.log_bytes
    }
    /// How long `request` rows are kept after their entry's `at`. Every member of a group must use the same value,
    /// because each drops rows as it applies. Default `REQUEST_RETENTION`.
    pub fn set_request_retention(&mut self, d: std::time::Duration) {
        self.fold.set_retention(d);
    }
    pub fn group_fold(&self) -> &GroupFold<C::Fold> {
        &self.fold
    }
    pub fn boot(&self) -> u64 {
        self.boot
    }

    /// The latest config in the log, committed or not, which counts votes and acks and names the members a host
    /// sends to; the bootstrap config if the log is empty.
    pub fn latest_config(&self) -> &GroupConfig {
        self.log_config()
    }

    /// The latest committed config and its index; `None` until the bootstrap entry commits.
    pub fn committed_config(&self) -> Option<&(Index, GroupConfig)> {
        self.fold.config()
    }

    pub fn not_leader(&self) -> NotLeader {
        // A leader under `no_lead_until` is handing off and names no leader, so the edge reports `suspect`.
        let leading = self.role == Role::Leader && !self.no_lead();
        NotLeader {
            term: self.state.term,
            leader: self.leader.filter(|l| *l != self.cfg.me || leading),
            config: self.fold.config().cloned(),
        }
    }

    pub fn meta(&self) -> ReplyMeta {
        ReplyMeta {
            term: self.state.term,
            leader: self.leader,
            config_index: self.fold.config().map_or(Index(0), |c| c.0),
        }
    }

    fn begin(&mut self, now: Timestamp) {
        self.now = now;
        self.step += 1;
        self.input_applied = self.fold.applied();
    }

    fn finish<R>(&mut self, reply: R) -> Ready<C, R> {
        self.check_remote();
        if self.dirty {
            self.dirty = false;
            self.state.saved_at = self.now;
            self.writes.push(Write::State(self.state.clone()));
        }
        if let Some(index) = self.own_appended.take() {
            self.out.self_ack = Some(SelfAck { boot: self.boot, epoch: self.epoch, index });
        }
        Ready { writes: std::mem::take(&mut self.writes), reply, out: std::mem::take(&mut self.out) }
    }

    /// The leader's AppendEntries go to `early`: they carry its term, which it stored before it asked for votes, and
    /// its log, whose own new entries it does not count until `self_ack`. Every other message waits for the writes.
    fn send(&mut self, to: SiteId, msg: Message<Ev<C>>) {
        let env = Envelope { from: self.cfg.me, to, msg };
        if matches!(env.msg, Message::Append(_)) && self.own_durable >= self.early_from {
            debug_assert_eq!(self.role, Role::Leader, "only a leader sends AppendEntries");
            self.out.early.push(env);
        } else {
            self.out.messages.push(env);
        }
    }

    /// The writes of the `Ready` that released `ack` are durable: this leader now counts its own copy of the entries
    /// through `ack`'s index, and may commit them.
    pub fn self_ack(&mut self, now: Timestamp, ack: SelfAck) -> Ready<C, ()> {
        self.begin(now);
        if ack.boot == self.boot && ack.epoch == self.epoch {
            let to = self.own_durable.max(ack.index.min(self.last()));
            if to > self.own_durable {
                self.own_durable = to;
                self.own_durable_at = now;
            }
            self.advance_commit();
            self.after_ack();
        }
        self.finish(())
    }

    /// The highest index this member counts as durable on its own storage.
    pub fn own_durable(&self) -> Index {
        self.own_durable
    }

    fn rand(&mut self) -> u64 {
        splitmix(&mut self.rng)
    }
}

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
