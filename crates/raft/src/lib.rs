//! The Raft group core (`build/BUILD-PLAN.md`, M1-WP2; `design/DESIGN.md`, Shape, "Chains", "The write path", and
//! "The read path").
//!
//! A sans-IO state machine for one group member. Every input (`Member::step` for a message, `Member::tick` for time,
//! `Member::commit`, `Member::id_block`, `Member::read`, `Member::campaign`, `Member::suspect`, `Member::transfer`, `Member::deps_read`,
//! and the health inputs `Member::call_failed` and `Member::confirm_end`) takes the caller's clock and returns a
//! `Ready`: storage writes, then the call's reply and the messages, ticket resolutions, read grants, dependency
//! reads, and alarms it produced. Only `Ready::persist` releases those, so a host makes the writes durable first:
//! that is the Durable Object output gate, stated as a type. One exception: the leader's AppendEntries (`Out::early`)
//! may leave before its own append is durable, and the leader counts its own copy only once the host returns the
//! input's `SelfAck` to `Member::self_ack` after the write confirms (Raft dissertation 10.2.1). No async, no wall
//! clock, and no randomness but the seed the host passes to `Member::new`.

pub mod admit;
pub mod codec;
mod edge;
mod encode;
mod fold;
mod member;
mod outcome;
mod quorum;
mod read;
mod types;

use std::time::Duration;

use graftyard_types::Timestamp;

pub use edge::{Receipt, Suspect};
pub use encode::{Enc, bootstrap, digest, encode_entry};
pub use fold::{Command, Ev, Fold, GroupFold, MemRequests, Record, RequestRow, RequestStore, Scratch};
pub use graftyard_types::{Digest, IdBlock, Index, RequestId, Ulid};
pub use member::{
    APPEND_BYTES_MAX, AdmissionView, Alarm, AppendReq, AppendRes, ByRequest, CallFailure, Catchup, ConfirmId,
    ConfirmTimes, CutEnd, CutNote, DONE_BYTES, Deny, DepsUsed, DepsWanted, Envelope, FailedCall, FoldView,
    IdBlockError, InstallError, InstallSnapshot, InstallSource, InstallWanted, LEARNER_READ_EVERY, LEARNERS_OUT_MAX,
    LogBase, Member, MemberConfig, Message, OUT_KEEP, OUT_MAX, OUTCOME_KEEP, Out, PEER_OUT_MAX, PIPELINE,
    READ_PIPELINE, READ_RESEND, READ_ROUND_GAP, ReadId, ReadStart, Ready, ReconfigError, Refusal, Resolution,
    RestoreError, Role, SelfAck, StepDownWhy, StepDownWindow, Storage, Stored, Verdict, VoteReq, VoteRes, Write,
    verify,
};
pub use outcome::Acked;
pub use quorum::{DurableAck, QuorumProof};
pub use read::{Fresh, ReadGrant, ReadView, Stale};
pub use types::{
    Admitted, CallError, Colo, CommitError, Committed, EntryBody, ExtendLeases, Format, GroupConfig, LogEntry,
    MemberState, NotLeader, PREV_ZERO, RAFT_FORMAT, ReleaseTag, ReplyMeta, Term, Ticket, TierSet, TransferError,
    TransferId, Unsupported,
};

/// Quorum rules, exposed for hosts and tests.
pub mod quorums {
    pub use crate::quorum::{commit, election, is_voter, learners, voters};
}

/// Leader serves reads alone while it holds this.
pub const LEASE: Duration = Duration::from_secs(9);
/// Follower may campaign only after its promise lapses.
pub const ELECTION_AFTER: Duration = Duration::from_secs(10);
/// Subtracted from the leader's own lease end [guess].
pub const DRIFT: Duration = Duration::from_millis(500);
/// The shortest stall a leader stamps as `ExtendLeases` (`design/DESIGN.md`, Shape, `ExtendLeases`) [guess].
pub const EXTEND_MIN: Duration = Duration::from_secs(2);
/// Receipt and member-to-member calls.
pub const CALL_DEADLINE: Duration = Duration::from_secs(1);
/// Per commit-path member call.
pub const COMMIT_RETRIES: u8 = 2;
/// Pre-vote refuses a candidate whose two `boots`, or whose `stepped_down_at`, fall inside it. A voter that has seen
/// no leader for 2 x ELECTION_AFTER waives it.
pub const RESET_QUARANTINE: Duration = Duration::from_secs(300);
/// A leader steps down when, over `window_s`, its commit-path error rate passes `commit_errors` to every follower with
/// at least `commit_fail_min` failed calls to each, or at least `confirm_slow_min` of its own storage confirms pass
/// `confirm_p99_ms` and so does the window's p99 (`design/DESIGN.md`, Shape, \"Chains\", `Health`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Health {
    pub window_s: u16,
    pub commit_errors: f32,
    pub commit_fail_min: u8,
    pub confirm_p99_ms: u32,
    pub confirm_slow_min: u8,
}

/// [measured, B0-06; commit_fail_min M0-WP4 min3]
pub const STEP_DOWN: Health =
    Health { window_s: 10, commit_errors: 0.05, commit_fail_min: 3, confirm_p99_ms: 500, confirm_slow_min: 6 };
/// A leader whose event loop is behind steps down when at least `.0` syncs in `.1` ended storage confirms that took `.2`
/// ms or more and an exact multiple of 100 ms on the isolate's clock (`ConfirmTimes::isolate_ms`). While an isolate is
/// behind, workerd clamps `Date.now()` to the deadline of the overdue 100 ms tick, so a confirm's begin and end both
/// read tick deadlines. Over 82 one-minute B0-02 runs this rule matched 11 of 25 failing runs and 1 of 57 passing ones,
/// at 600 ms (`tests/m0/results/carried-8.md`). The member clock, raised by peer stamps, is not on the tick, and the
/// confirms one sync ends are one span (`tests/m0/results/confirm-storage-m3f.md`, B0-29 m3fp2).
pub const LOOP_LAG: (u32, Duration, u32) = (2, Duration::from_secs(2), 200);
/// Distinct colos reporting, and the window.
pub const SUSPECT: (usize, Duration) = (2, Duration::from_secs(10));
/// [guess]
pub const COMMIT_HOLD: Duration = Duration::from_secs(1);
/// [guess]; above the 24 h orphan sweep.
pub const REQUEST_RETENTION: Duration = Duration::from_secs(48 * 3600);
/// Applied entries a Durable Object member keeps in memory (`Member::set_log_window`); older ones are read from
/// storage. A member holds every entry in memory otherwise, which a 128 MB isolate cannot do for long
/// (`tests/m1/results/m1-wp2.md`, the 24-hour run). Request rows are the `RequestStore`'s: a host keeps them on
/// storage.
pub const LOG_WINDOW: u64 = 1_000;
/// Bytes of applied entries (their canonical encoding) a Durable Object member keeps in memory, and `load` reads
/// back, beside `LOG_WINDOW` [guess]: 1,000 entries of up to `ENTRY_MAX` would be 1.5 GB against a 128 MB isolate,
/// and a member with about 50 MB of inline packs in its window ran out of memory in `load` (F45,
/// `tests/m1/results/m1-f43.md`). Entries not yet applied are held whatever their size.
pub const LOG_BYTES: usize = 8_000_000;
/// The largest encoded command, in bytes [guess].
pub const COMMAND_MAX: usize = 1_200_000;
/// The largest `Events` entry: "Every command that arrives during a round goes into the next entry, up to 1.5 MB".
pub const ENTRY_MAX: usize = 1_500_000;
/// Where the leader stops filling an `Events` entry: past its first command, a command joins only while the entry
/// stays within this many encoded bytes, not counting Agent inline packs (`AGENT_INLINE_ENTRY` bounds those). A cut
/// takes the whole queue up to here, so the bound binds only on a backlog: at light load an entry carries what arrived
/// during one round, and B0-02's at 300 commands/s were 33 to 50 commands. A Durable Object member's storage confirm
/// grows with the entry: confirm p99 was 95 to 132 ms for 20 to 80 KB entries, 168 ms at 80 to 100 KB, and 300 ms at
/// 100 to 130 KB (`tests/m0/results/carried-6.md`). At 60 KB, one entry a round held a backlog to 322 to 331
/// commands/s at B0-02's near round of 169 to 174 ms, and two gate runs at 330 offered queued past 600 ms. Entries of
/// 90 to 110 KB carried about 92 commands in a near round of p50 234 ms, 393 commands/s, a utilization of 0.78 at 308
/// committed (`tests/m0/results/carried-8.md`) [measured].
pub const ENTRY_SOFT: usize = 100_000;
/// A boot more than this long after the member last saved its state is an idle eviction, not a reset
/// (`MemberState.boots`).
pub const BOOT_IDLE: Duration = Duration::from_secs(5);

pub(crate) fn after(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp(t.0.saturating_add(d.as_millis() as u64))
}
