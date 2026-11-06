//! Shape types for chains and the write path (`design/DESIGN.md`, Shape, "Chains" and "The write path").

use std::fmt;
use std::ops::Range;
use std::time::Duration;

pub(crate) use graftyard_types::{Digest, Index, RequestId};
use graftyard_types::{IdBlock, Placement, ReleaseId, SiteId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::fold::Command;
use crate::outcome::Acked;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Term(pub u64);

/// `graftyard_types::Index`, with the step this crate takes everywhere.
pub(crate) trait Next {
    fn next(self) -> Index;
}

impl Next for Index {
    fn next(self) -> Index {
        Index(self.0 + 1)
    }
}

/// The `prev` of index 1. An entry's `Digest` (`graftyard_types::Digest`) is SHA-256 of its canonical encoding
/// (`crate::encode`), and `prev` chains them.
pub const PREV_ZERO: Digest = Digest([0; 32]);

/// One stored record format, as `LogEntry.format` names it: a record family and its version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Format {
    pub family: u16,
    pub version: u16,
}

/// The format of `Noop` and `Config` entries, which every release reads.
pub const RAFT_FORMAT: Format = Format { family: 0, version: 1 };

/// The fold tiers an entry touches (`design/SHARED.md`, `tier_of`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TierSet(pub u8);

impl TierSet {
    pub const NONE: TierSet = TierSet(0);
    pub const CORE: TierSet = TierSet(1);
    pub const METADATA: TierSet = TierSet(2);

    pub fn union(self, other: TierSet) -> TierSet {
        TierSet(self.0 | other.0)
    }
}

/// A leadership transfer's id (`design/DESIGN.md`, Shape, "Leadership"), drawn from the member's seeded generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransferId(pub u64);

/// The data center an edge runs in, as the host names it (for example an IATA code packed into the integer). `suspect`
/// counts reports per distinct colo. The design names the type `Colo` without defining it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Colo(pub u32);

/// Why `Member::transfer` did not start a transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    NotLeader(NotLeader),
    /// A transfer is already running.
    Busy(TransferId),
    /// `to` is this member, not a voter, drained, unable to follow, or quarantined.
    Target(SiteId),
}

/// The recorded stall (`design/DESIGN.md`, Shape, `ExtendLeases`). No leader in M1-WP2 sets one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtendLeases {
    pub since: Timestamp,
    pub by: Duration,
}

/// The release a member runs. `seq` orders releases, so a refusal can say whether the refuser runs an older release
/// than the leader (`design/DESIGN.md`, `verify`); the `Release` group's record order supplies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTag {
    pub id: ReleaseId,
    pub seq: u32,
}

/// Row in each member's `log` table, keyed by index. `prev` chains digests, so a gap or a changed entry is refused.
/// `requests` maps each request id to the range of its events in `body`, so a duplicate is answered from them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry<E> {
    pub term: Term,
    pub index: Index,
    pub prev: Digest,
    pub format: Format,
    pub tiers: TierSet,
    pub at: Timestamp,
    pub requests: SmallVec<[(RequestId, Range<u32>); 4]>,
    pub extend: Option<ExtendLeases>,
    pub body: EntryBody<E>,
}

/// The design's `EntryBody` also has `Release`, `Archived`, `Promoted`, `Split`, and `Imported`. Their payload types
/// belong to later work packages, so they are not here yet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EntryBody<E> {
    /// <= 1.5 MB including inline packs.
    Events(Vec<E>),
    Config(GroupConfig),
    /// The first entry of every term.
    Noop,
    /// Ids the cell's `Ids` group leased to this group, recorded before the group assigns from them. Carries no
    /// requests, and starts at or after the end of the group's previous block (`verify`). Applied to the fold with
    /// `Fold::apply_id_block`; appended by `Member::id_block`.
    IdBlock(IdBlock),
}

/// Membership is in the log. Under `Joint`, a lease and a read-index round need promises from a majority of both
/// configs. Pre-vote runs before every campaign, and votes are refused to non-members of the latest committed config.
///
/// The placements are boxed: a `Placement` holds up to 16 learners inline, about 1 KB, and every `NotLeader` and
/// error carries a config. Its JSON form is `{"normal": placement}`, `{"degraded": {placement, lost}}`, or
/// `{"joint": {old, new}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupConfig {
    Normal(Box<Placement>),
    Degraded { placement: Box<Placement>, lost: SiteId },
    Joint { old: Box<Placement>, new: Box<Placement> },
}

/// One row per member, written before any reply leaves (DO output gate), so a restart never forgets a vote or a
/// promise. The first eleven fields are the design's. `promised_to` and `saved_at` are added: see their comments.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberState {
    pub term: Term,
    pub voted_for: Option<SiteId>,
    pub commit: Index,
    pub applied: Index,
    /// No vote for a rival until then.
    pub promise_until: Timestamp,
    /// A restarted leader resumes this term after one round.
    pub led_term: Option<Term>,
    /// An older follower refused our uncommitted entry: the release we ran then. While it equals our release this
    /// member neither leads nor takes a transfer. It clears at boot when our release changes; the `Release` group's
    /// rollback of the refuser's site is the host's to apply (`Member::clear_no_lead`).
    pub no_lead_until: Option<ReleaseId>,
    /// Same-or-newer refusers, until their release changes.
    pub cannot_follow: SmallVec<[SiteId; 2]>,
    /// The leader's transfer in progress, or the voter's last accepted one.
    pub transfer: Option<TransferId>,
    /// Cleared when an entry carrying `extend` commits. Never set yet (`ExtendLeases` is later work).
    pub stalled_since: Option<Timestamp>,
    /// The last two boots that interrupted work. A boot more than `BOOT_IDLE` after `saved_at` is an idle eviction
    /// and is not counted.
    pub boots: [Timestamp; 2],
    /// Our last leadership ended by STEP_DOWN then.
    pub stepped_down_at: Option<Timestamp>,
    /// The leader `promise_until` was given to. A promise bars votes for a rival of that leader, not for that leader,
    /// so the row must say who it is: after a restart the member no longer knows who led. Not in the design's row.
    pub promised_to: Option<SiteId>,
    /// When this row was last written, for the `boots` rule ("a boot more than 5 s after the last saved state").
    /// Not in the design's row, which the rule needs.
    pub saved_at: Timestamp,
    /// The leader's `sent_at` on the AppendEntries whose `transfer` this voter last took into `transfer`. A delayed
    /// append from before an abandoned transfer must not bring its id back after a later `transfer: None` round, or
    /// the voter could break a promise the leader's new lease counts on. Not in the design's row.
    pub transfer_at: Timestamp,
    /// Counts this member's leaderships, new and resumed; AppendEntries carry it. A resumed `led_term` stores it before
    /// any of its messages leave. Not in the design's row.
    pub lead_seq: u64,
    /// The highest `lead_seq` of an AppendEntries this member accepted in `term`, 0 on a term change. An append of
    /// this term from an older instance is refused: that instance may have sent an entry before its own write was
    /// durable and lost it, and a delayed copy would truncate what its successor in the same term committed. Not in
    /// the design's row.
    pub follow_seq: u64,
}

impl MemberState {
    /// A member that has never stored anything: term 1, which only the bootstrap entry occupies.
    pub fn fresh() -> MemberState {
        MemberState {
            term: Term(1),
            voted_for: None,
            commit: Index(0),
            applied: Index(0),
            promise_until: Timestamp(0),
            led_term: None,
            no_lead_until: None,
            cannot_follow: SmallVec::new(),
            transfer: None,
            stalled_since: None,
            boots: [Timestamp(0); 2],
            stepped_down_at: None,
            promised_to: None,
            saved_at: Timestamp(0),
            transfer_at: Timestamp(0),
            lead_seq: 0,
            follow_seq: 0,
        }
    }
}

/// `boot` names the leader instance that issued the ticket, drawn at random when the object starts, so a leader that
/// resumes `led_term` after a reset never answers a ticket from before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ticket {
    pub term: Term,
    pub boot: u64,
    pub seq: u64,
}

/// Any reply a member gives when it cannot lead the call. `config` is its latest committed config, or `None` when it
/// has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotLeader {
    pub term: Term,
    pub leader: Option<SiteId>,
    pub config: Option<(Index, GroupConfig)>,
}

/// Carried by every member reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyMeta {
    pub term: Term,
    pub leader: Option<SiteId>,
    pub config_index: Index,
}

/// A member that cannot decode a message answers this and changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsupported {
    pub release: ReleaseId,
    pub format: Format,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    NotLeader(NotLeader),
    Unsupported(Unsupported),
    /// The encoded command is over `COMMAND_MAX`. The edge refuses these before `commit`; the member refuses them
    /// again rather than decide one. Not in the design's `CallError`.
    TooLarge {
        len: usize,
        max: usize,
    },
}

/// How a ticket ends without an `Acked`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError<E> {
    /// `decide` refused, and the entry the refusal rests on committed at `index`.
    Refused { index: Index, err: E },
    /// The leadership that held the ticket ended, or the ticket is from another term or boot. The caller re-sends
    /// with the same request id.
    NotLeader(NotLeader),
    /// An Agent-class command waited AGENT_WAIT in its lane and never entered an entry (`crate::admit`). Git gets
    /// `ng <ref> agent admission full, retry after <n>s`, REST 429 with `retry-after`. A re-send under the same request
    /// id is admitted afresh.
    Full { retry_after_s: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Admitted {
    pub ticket: Ticket,
    /// The class the leader queued it under: the edge's, or Agent after DEMOTE_PER_MIN. A command joining a ticket
    /// already queued reports the class it was first sent with.
    pub class: crate::admit::AdmitClass,
}

/// What a held `commit` answers: the outcome when the ticket resolved within COMMIT_HOLD, else the ticket.
pub enum Committed<C: Command> {
    Done(Result<Acked<C::Out>, CommitError<C::Err>>),
    Admitted(Admitted),
}

impl<C: Command> fmt::Debug for Committed<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Committed::Done(r) => f.debug_tuple("Done").field(r).finish(),
            Committed::Admitted(a) => f.debug_tuple("Admitted").field(a).finish(),
        }
    }
}

impl<C: Command> Clone for Committed<C> {
    fn clone(&self) -> Self {
        match self {
            Committed::Done(r) => Committed::Done(r.clone()),
            Committed::Admitted(a) => Committed::Admitted(*a),
        }
    }
}
