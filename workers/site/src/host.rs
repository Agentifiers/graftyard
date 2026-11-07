//! The Raft core hosted in a member Durable Object (`build/BUILD-PLAN.md`, M1-WP2).
//!
//! - Storage. `Ready::persist` hands every `Write` to `SqlStore`, which turns it into SQLite statements on the
//!   object's own database: `raft_state` (the `MemberState` row), `log` (one row per index, with its digest), and on
//!   `Apply` the fold (`reg`) and the core-tier `request` table. One input's statements run in one
//!   `storage.transactionSync`, so they land together or not at all, and the Durable Object output gate holds every
//!   reply and subrequest issued after them until they are durable. That is the crate's persist-then-send rule on the
//!   platform: nothing `persist` returns leaves before the writes it depends on.
//! - Restore. The constructor reads the rows back, rehashes every entry against its stored digest, rebuilds the
//!   `GroupFold` by applying the log through `MemberState.applied` (the crate has no way to load a `GroupFold` from
//!   its tables), checks the rebuild against the `reg` and `request` tables, and calls `Member::restore`.
//! - Transport. Each `Envelope` is one POST of the binary envelope to the peer member's `/raft` through its site's
//!   binding, with CALL_DEADLINE. The peer steps it and answers with its envelopes for the caller in the response
//!   body, which its output gate also holds until its writes are durable. A call that errors or misses CALL_DEADLINE
//!   goes to `Member::call_failed`, and the crate's `tick` resends, so the host never retries a message itself. A call
//!   that missed CALL_DEADLINE is still awaited for LATE_LIMIT, and its answer, if it comes, is stepped (`answer`).
//!   Every call's outcome, and the cause of each failure, is kept per peer for `/stats` (`PeerCalls`).
//! - Time. `tick` runs every TICK while an input arrived in the last ACTIVE_FOR_MS or a call is held, and from the
//!   one alarm the design allows: a leader's, every 2 s while it served a request in the last 60 s (`design/DESIGN.md`,
//!   Shape, the drain check). `tick` starts no campaign; campaigns start only from `/campaign` and `/suspect`.
//! - Faults. With the `test-faults` feature only, `crate::faults` adds BF-12's and BF-13's kills, the call faults of
//!   the 24-hour run, and the harness's tables (`fault`, `dup`, `deps_log`). A product build has none of them.

use std::cell::{Cell, RefCell};
#[cfg(feature = "test-faults")]
use std::collections::BTreeSet;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::rc::Rc;
use std::time::Duration;

use futures_channel::oneshot;
use futures_util::future::{Either, select};
use graftyard_raft::{
    self as raft, Acked, CallError, CommitError, EntryBody, GroupFold, Index, MemberConfig, NotLeader, Out, ReadId,
    Ready, ReleaseTag, RequestId, Resolution, Role, Storage, Stored, Ticket, Write, codec,
};
use graftyard_types::{ReleaseId, SiteId, Timestamp};
use serde_json::{Value, json};
use worker::js_sys;
use worker::wasm_bindgen::closure::Closure;
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::{Err as FErr, Family, Out as FOut, Stmt};
use crate::member::Join;
use crate::{SiteEnv, now_ms};

type Entry<F> = raft::LogEntry<<F as Family>::Ev>;
type Envelope<F> = raft::Envelope<<F as Family>::Ev>;

pub type Outcome<F> = Result<Acked<FOut<F>>, CommitError<FErr<F>>>;
pub type ReadResult = Result<(Value, Index), NotLeader>;

const TICK: Duration = Duration::from_millis(100);
const ACTIVE_FOR_MS: u64 = 3_000;
const DRAIN_CHECK_MS: i64 = 2_000;
const DRAIN_IDLE_MS: u64 = 60_000;
/// Restore compares the fold loaded from its tables with the log replayed through `applied`, while at most this many
/// entries are applied. Longer logs skip it: replaying them would hold every request row the log ever carried.
const RESTORE_CHECK_MAX: u64 = 20_000;
/// Latency samples kept for `/stats`.
const SAMPLES_MAX: usize = 200_000;
/// Storage confirms and truncations kept for `ack_json`'s `selfSyncedAt`: an `Acked` is answered within seconds of
/// its commit, and a confirm row a minute old covers every index at or below it.
const SYNCED_KEEP: usize = 256;
/// Stored bytes per `/entries` page. 239 entries of `wp6/main` were 4.6 MB of JSON and took 7.8 s to answer through
/// the test edge, past the lookup's 3.5 s CALL, so the git cache shard's rehydrate never finished
/// (`tests/m1/results/m1-wp8.md`, t-256). A page this size is under 1 MB of JSON [guess for the bound].
const ENTRIES_PAGE_BYTES: usize = 600_000;

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS raft_state (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, digest BLOB NOT NULL, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS reg (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
    crate::release::RELEASE_SCHEMA,
    "CREATE TABLE IF NOT EXISTS fold (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS id_block (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS group_config (id INTEGER PRIMARY KEY CHECK (id = 1), idx INTEGER NOT NULL, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS request (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, at INTEGER NOT NULL, events BLOB NOT NULL)",
    // Every apply deletes the rows past retention by `at`.
    "CREATE INDEX IF NOT EXISTS request_at ON request (at)",
    // The base an installed snapshot left (`Write::Install`): the log holds no row at or below it.
    "CREATE TABLE IF NOT EXISTS log_base (id INTEGER PRIMARY KEY CHECK (id = 1), idx INTEGER NOT NULL, term INTEGER NOT NULL, digest BLOB NOT NULL)",
    // Every archive an applied `Archived` named, for the leader's `InstallSnapshot` source (`crate::load`).
    "CREATE TABLE IF NOT EXISTS archive_keys (from_idx INTEGER PRIMARY KEY, to_idx INTEGER NOT NULL, to_at INTEGER NOT NULL, key TEXT NOT NULL)",
];

#[cfg(feature = "test-faults")]
use crate::faults::{Fault, Kill};

#[derive(Default)]
pub struct Stats {
    /// Leader: a command's admission to its ticket's resolution on the commit-advance path, in ms.
    pub commit_ms: Vec<u32>,
    /// Leader: an `Events` entry's append to its apply, in ms.
    pub round_ms: Vec<u32>,
    /// Every member: each input that applied entries, as (its synchronous ms, from the member's `step` to its
    /// statements built; the entries it applied; their events) (BF-20's apply time).
    pub apply_steps: Vec<(u32, u32, u32)>,
    /// Every member: each entry's age when this member applied it, its `at` (the leader's clock at append) to this
    /// member's clock at apply, in ms. On a follower it includes the commit round and the wait for the next
    /// AppendEntries that carries the commit (BF-20).
    pub apply_ms: Vec<u32>,
    /// Leader: storage confirm of an input that appended an entry (its writes to `storage.sync()`), in ms. The output
    /// gate holds the entry's AppendEntries for this long.
    pub confirm_ms: Vec<u32>,
    /// Leader: one AppendEntries call with entries, send to reply, per follower site, in ms.
    pub append_ms: BTreeMap<u8, Vec<u32>>,
    pub entries: u32,
    pub commands: u32,
    pub restores: u32,
    /// Snapshots this member installed (`crate::load`).
    pub installs: u32,
    /// Requests this member received, by route, since `inbound_since` (B0-18's leader load).
    pub inbound: BTreeMap<String, u32>,
    pub inbound_since: u64,
    pub failed_calls: u32,
    pub dep_reads: u32,
    /// Stored entries sent to a follower behind the in-memory window (`Out::catchup`).
    pub catchups: u32,
    pub leases: u32,
    /// `Committed` marker puts that failed after an ok push (the sweeper repairs them).
    pub marker_failures: u32,
    /// The stored log's first row after the last truncation (`truncate`).
    pub truncated_to: u64,
    /// Commands this member received, by format (`recv:<family>.<version>`), and `recv:unsupported` for bodies whose
    /// command it could not decode.
    pub received: BTreeMap<String, u32>,
    pub alarms: Vec<String>,
    /// Object alarms that fired, and when the last one did.
    pub alarm_runs: u32,
    pub alarm_last: u64,
    pub alarm_arms: u32,
    /// `PrSync` batches this member's leader alarm sent to its site's queue, the re-send path.
    pub prsync_sent: u32,
    /// `PrSync` first attempts this member's leader alarm ran in band.
    pub prsync_first: u32,
    appended: Vec<(u64, u64)>,
    /// Every member-to-member call this member sent, by the peer's site id.
    pub peers: BTreeMap<u8, PeerCalls>,
    /// Every member-to-member call this member sent, by message kind: send to answer, in ms (B0-18's trace).
    pub calls: BTreeMap<&'static str, CallTimes>,
    /// Test installs: B0-18's read-index trace, oldest first, each `[at, kind, peer, batch, index, ms]` (`Trace`).
    pub trace: VecDeque<[u64; 6]>,
    /// Test installs: member-to-member calls by modelled hop (`Host::hop`): `[ok, failed for depth, failed otherwise]`.
    pub hops: BTreeMap<u64, [u32; 3]>,
}

/// The kinds in `Stats::trace`. `peer` is the other member's address; `index` is the read index (0 for none), the
/// applied index, or a call's end (0 ok, 1 late, 2 failed, 3 failed late); `ms` is a call's time or a lag.
#[derive(Clone, Copy)]
pub enum Trace {
    /// A `ReadIndex` left, to the leader.
    IndexSent = 1,
    /// That call ended.
    IndexEnded = 2,
    /// A `ReadIndexRes` was stepped here: `ms` 0 from the answer to this member's own call, 1 from a call the leader
    /// made.
    ResStepped = 3,
    /// This member's applied index moved to `index`.
    Applied = 4,
    /// The leader took a `ReadIndex`.
    IndexTaken = 5,
    /// The leader's `ReadIndexRes` left: `ms` 0 in the answer to the asker's call, 1 as its own call.
    ResSent = 6,
    /// The leader's own `ReadIndexRes` call ended.
    ResEnded = 7,
    /// The member's 100 ms ticker woke `ms` late.
    Lag = 8,
    /// A call to `peer` failed "Subrequest depth limit exceeded": `batch` is its message kind's `Call::msg` index,
    /// `index` its modelled hop (`Host::hop`).
    DepthFail = 9,
}

/// Trace entries kept per member: 16 learners' batches at 20 a second for a 30 s run, on the leader, twice over.
#[cfg(feature = "test-faults")]
const TRACE_MAX: usize = 60_000;

/// One message kind's calls: how many, how they ended, their times, and the slowest, newest last.
#[derive(Default, serde::Serialize)]
pub struct CallTimes {
    pub ok: u32,
    pub failed: u32,
    pub ms: Vec<u32>,
    /// Calls over `SLOW_CALL_MS`, each `{at, to, ms, end}`.
    pub slow: VecDeque<Value>,
}

/// A call this slow is kept in `CallTimes::slow` [guess]: past the learner read gate's 1 s p99 less a round trip.
const SLOW_CALL_MS: u32 = 500;

/// Why a member-to-member call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallFail {
    /// No member of the group on that site, by the latest config or the bootstrap record.
    Peer,
    SiteEnv,
    Stub,
    Request,
    /// The stub's `fetch` threw.
    Fetch,
    /// The peer answered, not 200.
    Status,
    Body,
    Decode,
    /// No answer within CALL_DEADLINE.
    Deadline,
    #[cfg(feature = "test-faults")]
    Injected,
}

impl CallFail {
    fn name(self) -> &'static str {
        match self {
            CallFail::Peer => "peer",
            CallFail::SiteEnv => "siteEnv",
            CallFail::Stub => "stub",
            CallFail::Request => "request",
            CallFail::Fetch => "fetch",
            CallFail::Status => "status",
            CallFail::Body => "body",
            CallFail::Decode => "decode",
            CallFail::Deadline => "deadline",
            #[cfg(feature = "test-faults")]
            CallFail::Injected => "injected",
        }
    }
}

/// A failed call's cause and the error text it carried.
pub struct PeerErr {
    kind: CallFail,
    msg: String,
}

impl PeerErr {
    fn new(kind: CallFail, msg: impl ToString) -> PeerErr {
        let mut msg = msg.to_string();
        if msg.len() > PEER_MSG_MAX {
            let mut cut = PEER_MSG_MAX;
            while !msg.is_char_boundary(cut) {
                cut -= 1;
            }
            msg.truncate(cut);
        }
        PeerErr { kind, msg }
    }
}

/// Recent errors and AppendEntries calls kept per peer: enough to read a stall's cause from `/stats`, few enough to
/// keep in every member.
const PEER_ERRORS: usize = 8;
const PEER_APPENDS: usize = 32;
const PEER_MSG_MAX: usize = 200;

/// The calls this member sent to one peer. A call that misses CALL_DEADLINE counts under `failed` as `deadline`, and
/// once more under `late` if its answer then arrives (which is stepped) or `lateFailed` if it then fails.
#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerCalls {
    /// Answered within CALL_DEADLINE.
    pub ok: u32,
    pub failed: BTreeMap<&'static str, u32>,
    pub late: u32,
    pub late_ms_max: u32,
    /// Missed CALL_DEADLINE, then failed, by cause.
    pub late_failed: BTreeMap<&'static str, u32>,
    /// Newest last, each `{at, kind, msg, call, entries, bytes, ms}`.
    pub errors: VecDeque<Value>,
    /// AppendEntries calls with entries, newest last, each `{at, round, first, last, bytes, ms, end}` (`first` and
    /// `last` are entry indexes). A call that missed its deadline shows twice: `deadline` then, and `late` or its
    /// error when it ended.
    pub appends: VecDeque<Value>,
}

/// One call as sent: to whom, what it carried, and when.
struct Call {
    to: u8,
    msg: &'static str,
    /// An AppendEntries call's round and the indexes of its first and last entry.
    append: Option<(u64, u64, u64)>,
    entries: usize,
    bytes: usize,
    at: u64,
    /// A `ReadIndex` or `ReadIndexRes` call's batch.
    batch: Option<u64>,
    /// The call's modelled hop (`Host::hop`), set when it leaves.
    hop: Cell<u64>,
}

impl Call {
    fn of<E>(e: &raft::Envelope<E>, bytes: usize) -> Call {
        let msg = match &e.msg {
            raft::Message::Append(_) => "append",
            raft::Message::AppendRes(_) => "appendRes",
            raft::Message::Vote(_) => "vote",
            raft::Message::VoteRes(_) => "voteRes",
            raft::Message::Propose(_) => "propose",
            raft::Message::Proposed { .. } => "proposed",
            raft::Message::TimeoutNow { .. } => "timeoutNow",
            raft::Message::TransferRefused { .. } => "transferRefused",
            raft::Message::Probe { .. } => "probe",
            raft::Message::ProbeRes { .. } => "probeRes",
            raft::Message::Ping { .. } => "ping",
            raft::Message::Pong { .. } => "pong",
            raft::Message::Install(_) => "install",
            raft::Message::ReadIndex { .. } => "readIndex",
            raft::Message::ReadIndexRes { .. } => "readIndexRes",
        };
        let (append, entries) = match &e.msg {
            raft::Message::Append(a) => {
                (Some((a.round, a.prev_index.0 + 1, a.prev_index.0 + a.entries.len() as u64)), a.entries.len())
            }
            _ => (None, 0),
        };
        let batch = match &e.msg {
            raft::Message::ReadIndex { batch } | raft::Message::ReadIndexRes { batch, .. } => Some(*batch),
            _ => None,
        };
        Call { to: e.to.0, msg, append, entries, bytes, at: now_ms(), batch, hop: Cell::new(0) }
    }
}

/// How a call ended.
enum End {
    Ok,
    /// Answered after CALL_DEADLINE, within LATE_LIMIT more.
    Late,
    Failed(PeerErr),
    /// Missed CALL_DEADLINE, then failed.
    LateFailed(PeerErr),
}

/// How a call ended against its deadline.
#[derive(Debug, PartialEq)]
pub enum Answer<T> {
    InTime(T),
    /// The deadline passed first, and the call ended before `limit`.
    Late(T),
    Missed,
}

/// Races `call` against `deadline`. A call that misses it runs `missed` at once, so the Raft core hears of the miss
/// at its deadline, and is then awaited until `limit`. Its answer, if it comes, is returned as `Late`, for the core to
/// step (`Member::call_failed`: "A reply that still arrives later may be passed to `step`"). A follower whose
/// AppendEntries take longer than CALL_DEADLINE to transfer and apply answers only late, and a leader that drops
/// those answers never learns it caught up, so it resends the same entries forever (`tests/m1/results/m1-f43.md`).
pub async fn answer<T>(
    call: impl Future<Output = T>,
    deadline: impl Future<Output = ()>,
    limit: impl Future<Output = ()>,
    missed: impl FnOnce(),
) -> Answer<T> {
    let mut call = Box::pin(call);
    match select(call.as_mut(), Box::pin(deadline)).await {
        Either::Left((v, _)) => Answer::InTime(v),
        Either::Right(_) => {
            missed();
            match select(call, Box::pin(limit)).await {
                Either::Left((v, _)) => Answer::Late(v),
                Either::Right(_) => Answer::Missed,
            }
        }
    }
}

/// How long past CALL_DEADLINE a call is still awaited [guess]: the slowest AppendEntries a follower answered in the
/// F43 logs took 2.0 s on the callee (`tests/m1/results/m1-f43.md`).
const LATE_LIMIT: Duration = Duration::from_secs(10);

pub struct Core<F: Family> {
    pub m: raft::Member<F::C>,
    pub join: Join,
    applied: Index,
    waiters: HashMap<Ticket, Vec<oneshot::Sender<Outcome<F>>>>,
    /// The request id, admission time, and command of each ticket this boot admitted, for kills, latency, and the
    /// id-block lease loop's re-send.
    admitted: HashMap<Ticket, (RequestId, u64, Option<F::C>)>,
    reads: HashMap<ReadId, (Value, oneshot::Sender<ReadResult>)>,
    #[cfg(feature = "test-faults")]
    pub kills: HashMap<RequestId, (Kill, u64)>,
    /// A ticket refused `NeedIdBlock` and re-sent after the lease loop recorded a block: old ticket to new.
    pub reissued: HashMap<Ticket, Ticket>,
    /// Waiters for `Resolution::IdBlock`.
    blocks: Vec<(raft::IdBlock, oneshot::Sender<Result<Index, NotLeader>>)>,
    /// Waiters for `Resolution::Recorded`, by request id.
    recorded: Vec<(RequestId, oneshot::Sender<Result<Index, NotLeader>>)>,
    /// Test fault for BF-19: the request ids whose `Committed` marker puts the leader skips, as when it dies after
    /// `Acked` and before them.
    #[cfg(feature = "test-faults")]
    pub no_marker: std::collections::HashSet<RequestId>,
    /// Dependency reads to hold back, for BF-15's queued commands: until then.
    #[cfg(feature = "test-faults")]
    pub hold_deps_until: u64,
}

/// A storage confirm begun: its id, its start when the leader times it, and the input's `SelfAck`.
type Unconfirmed = (raft::ConfirmId, Option<u64>, Option<raft::SelfAck>);

pub struct Host<F: Family> {
    pub state: Rc<State>,
    pub env: Env,
    pub core: RefCell<Option<Core<F>>>,
    /// The group fold's `request` table on storage, which `flush` tells when the queued rows have landed.
    requests: RefCell<Option<RequestSql<F::Ev>>>,
    pub restore_error: RefCell<Option<String>>,
    pub dying: Cell<bool>,
    ticking: Cell<bool>,
    active_until: Cell<u64>,
    alarm_at: Cell<u64>,
    served_at: Cell<u64>,
    #[cfg(feature = "test-faults")]
    down_until: Cell<u64>,
    /// Statements of inputs whose transaction has not run yet, in input order. Every flush runs all of them, so
    /// transactions follow input order even when an input with early messages waits a turn.
    queue: RefCell<Vec<Stmt>>,
    /// A transaction is scheduled for the next macrotask (`defer_flush`).
    flush_due: Cell<bool>,
    /// Outputs of inputs whose writes wait for the deferred transaction, and responses waiting for it.
    held: RefCell<Vec<Out<F::C>>>,
    flush_waiters: RefCell<Vec<oneshot::Sender<()>>>,
    /// Confirms begun and `SelfAck`s waiting for the writes of the next `flush` to be durable.
    unconfirmed: RefCell<Vec<Unconfirmed>>,
    pub stats: RefCell<Stats>,
    /// The host's own record of its storage confirms, for each `Acked`'s evidence (`ack_json`): the log's last index
    /// once a transaction that appended was confirmed durable, and `now_ms()` then. A truncation caps every row.
    synced: RefCell<VecDeque<(u64, u64)>>,
    /// The last log index the queued transaction leaves, when it appends.
    queued_last: Cell<Option<u64>>,
    /// Truncations stepped, numbered, with the index each left the log at.
    truncs: RefCell<VecDeque<(u64, u64)>>,
    /// Commands refused `NeedIdBlock`, waiting for the lease loop: request id, command, and their waiters.
    blocked: RefCell<Vec<(Ticket, RequestId, F::C)>>,
    leasing: Cell<bool>,
    /// Messages for learners that are calling this member, held for their next call (`crate::callers`).
    pub callers: RefCell<crate::callers::Callers<Envelope<F>>>,
    /// Transfers to abandon: their `TimeoutNow` is dropped (`/transfer` with `abandon`).
    #[cfg(feature = "test-faults")]
    pub abandon: RefCell<BTreeSet<u64>>,
    /// Test installs, B0-18's depth model: the hop of the newest request this object received, as its sender's
    /// `x-graft-hop` header said, or 2 for a request from a Worker (the edge, the tools edge). The runtime charges
    /// every outgoing call to the newest incoming request, so a call leaves at this hop plus one.
    #[cfg(feature = "test-faults")]
    pub hop: Cell<u64>,
}

pub fn err(m: impl Into<String>) -> Error {
    Error::RustError(m.into())
}

pub async fn within<T>(d: Duration, f: impl Future<Output = T>) -> Option<T> {
    match select(Box::pin(f), Box::pin(Delay::from(d))).await {
        Either::Left((v, _)) => Some(v),
        Either::Right(_) => None,
    }
}

fn seed() -> u64 {
    let a = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    let b = (js_sys::Math::random() * 4_294_967_296.0) as u64;
    (a << 32) ^ b ^ now_ms()
}

/// The release this build is. Milestone 1 has no release registry, so every build names the same one; `Unsupported`
/// carries it.
pub const RELEASE: ReleaseTag = ReleaseTag { id: ReleaseId([1; 32]), seq: 1 };

pub fn member_config<F: Family>(site: &SiteEnv, join: &Join) -> MemberConfig {
    MemberConfig {
        chain: join.chain,
        // A learner's address is its slot's, not its site's (`graftyard_types::learner_addr`).
        me: join.me.addr(),
        record: join.record.clone(),
        regions: site.sites.0.iter().map(|s| (s.id, s.region)).collect(),
        release: RELEASE,
        reads: F::reads(),
    }
}

// ---- storage ----

pub fn int(v: &SqlStorageValue) -> Result<i64> {
    match v {
        SqlStorageValue::Integer(i) => Ok(*i),
        SqlStorageValue::Float(f) => Ok(*f as i64),
        other => Err(err(format!("expected an integer, got {other:?}"))),
    }
}

pub fn blob(v: &SqlStorageValue) -> Result<&[u8]> {
    match v {
        SqlStorageValue::Blob(b) => Ok(b),
        other => Err(err(format!("expected a blob, got {other:?}"))),
    }
}

pub fn text(v: &SqlStorageValue) -> Result<&str> {
    match v {
        SqlStorageValue::String(s) => Ok(s),
        other => Err(err(format!("expected text, got {other:?}"))),
    }
}

pub fn rows(sql: &SqlStorage, q: &str, args: Vec<SqlStorageValue>) -> Result<Vec<Vec<SqlStorageValue>>> {
    sql.exec(q, args)?.raw().collect()
}

pub fn i(v: u64) -> SqlStorageValue {
    SqlStorageValue::Integer(v as i64)
}

/// Collects one input's writes as SQL statements; `commit` runs them in one transaction.
struct SqlStore<'a, F: Family> {
    m: &'a raft::Member<F::C>,
    fold: &'a F::Fold,
    applied: Index,
    stmts: Vec<Stmt>,
    /// Archives the applied entries committed (`Family::archived`), for `truncate`.
    archived: Vec<(String, u64)>,
    /// A snapshot install's base (`Write::Install`), for `step`.
    installed: Option<raft::LogBase>,
}

impl<F: Family> Storage<F::Ev> for SqlStore<'_, F> {
    fn write(&mut self, w: &Write<F::Ev>) {
        match w {
            Write::State(s) => self
                .stmts
                .push(("INSERT OR REPLACE INTO raft_state (id, row) VALUES (1, ?)", vec![codec::to_bytes(s).into()])),
            Write::Append(e) => self.stmts.push((
                "INSERT OR REPLACE INTO log (idx, term, digest, row) VALUES (?, ?, ?, ?)",
                vec![i(e.index.0), i(e.term.0), raft::digest(e).0.to_vec().into(), codec::to_bytes(e).into()],
            )),
            Write::Truncate(x) => self.stmts.push(("DELETE FROM log WHERE idx >= ?", vec![i(x.0)])),
            // `Member::install` took the fold `crate::load` built in its scratch tables: the log goes, the base is kept
            // beside it, the config and id block are the install's, and the family swaps the scratch tables in. The
            // host then restores the member from these rows (`crate::load::finish`).
            Write::Install(b) => {
                self.stmts.push(("DELETE FROM log", vec![]));
                self.stmts.push((
                    "INSERT OR REPLACE INTO log_base (id, idx, term, digest) VALUES (1, ?, ?, ?)",
                    vec![i(b.index.0), i(b.term.0), b.digest.0.to_vec().into()],
                ));
                let gf = self.m.group_fold();
                if let Some((idx, c)) = gf.config() {
                    self.stmts.push((
                        "INSERT OR REPLACE INTO group_config (id, idx, row) VALUES (1, ?, ?)",
                        vec![i(idx.0), codec::to_bytes(c).into()],
                    ));
                }
                match gf.id_block() {
                    Some(blk) => self.stmts.push((
                        "INSERT OR REPLACE INTO id_block (id, row) VALUES (1, ?)",
                        vec![codec::to_bytes(&blk).into()],
                    )),
                    None => self.stmts.push(("DELETE FROM id_block", vec![])),
                }
                F::install_stmts(&mut self.stmts);
                self.applied = b.index;
                self.installed = Some(*b);
            }
            Write::Apply(to) => {
                let mut applied = Vec::new();
                let mut last_at = None;
                for idx in self.applied.0 + 1..=to.0 {
                    // Committed entries are never truncated, and `compact` keeps every entry the input applied, so the
                    // member's log after the input still holds them.
                    let e = self.m.entry_at(Index(idx)).expect("an entry this input applied is in memory");
                    // Rows more than REQUEST_RETENTION older than this entry go first, as `GroupFold::apply` drops
                    // them before it applies the entry.
                    if last_at != Some(e.at) {
                        let cut = e.at.0.saturating_sub(self.m.group_fold().retention().as_millis() as u64);
                        self.stmts.push(("DELETE FROM request WHERE at < ?", vec![i(cut)]));
                        last_at = Some(e.at);
                    }
                    if let EntryBody::Config(c) = &e.body {
                        self.stmts.push((
                            "INSERT OR REPLACE INTO group_config (id, idx, row) VALUES (1, ?, ?)",
                            vec![i(idx), codec::to_bytes(c).into()],
                        ));
                    }
                    if let EntryBody::IdBlock(b) = &e.body {
                        self.stmts.push((
                            "INSERT OR REPLACE INTO id_block (id, row) VALUES (1, ?)",
                            vec![codec::to_bytes(b).into()],
                        ));
                    }
                    if let Some(a) = F::archived(e) {
                        self.stmts.push((
                            "INSERT OR REPLACE INTO archive_keys (from_idx, to_idx, to_at, key) VALUES (?, ?, ?, ?)",
                            vec![i(a.from), i(a.to), i(a.to_at), a.key.clone().into()],
                        ));
                        self.archived.push((a.key, a.to));
                    }
                    applied.push(e);
                    let EntryBody::Events(evs) = &e.body else { continue };
                    for (rid, r) in &e.requests {
                        // The row's form of each event, as `GroupFold::apply` keeps it (`Fold::row_event`).
                        let row: Vec<F::Ev> = evs[r.start as usize..r.end as usize]
                            .iter()
                            .map(<F::Fold as raft::Fold>::row_event)
                            .collect();
                        let b = codec::to_bytes(&row);
                        let hex = rid.to_string();
                        // A request id applied at a second index is the double apply BF-12 counts.
                        #[cfg(feature = "test-faults")]
                        self.stmts.push((
                            "INSERT INTO dup (rid, idx) SELECT ?, ? WHERE EXISTS (SELECT 1 FROM request WHERE rid = ?)",
                            vec![hex.clone().into(), i(idx), hex.clone().into()],
                        ));
                        self.stmts.push((
                            "INSERT INTO request (rid, idx, at, events) VALUES (?, ?, ?, ?) ON CONFLICT (rid) DO NOTHING",
                            vec![hex.into(), i(idx), i(e.at.0), b.into()],
                        ));
                    }
                }
                if !applied.is_empty() {
                    F::store_fold(self.fold, &applied, &mut self.stmts);
                }
                self.applied = self.applied.max(*to);
            }
        }
    }
}

/// Runs the statements in one `storage.transactionSync`, which workers-rs 0.8.7 does not bind.
pub fn commit_tx(storage: &worker::Storage, stmts: Vec<Stmt>) -> std::result::Result<(), String> {
    if stmts.is_empty() {
        return Ok(());
    }
    let sql = storage.sql();
    let raw: &JsValue = storage.as_raw().as_ref();
    let f = js_sys::Reflect::get(raw, &JsValue::from_str("transactionSync"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        .ok_or("no transactionSync")?;
    let cb = Closure::once_into_js(move || -> std::result::Result<JsValue, JsValue> {
        for (q, a) in stmts {
            sql.exec(q, a).map_err(|e| JsValue::from_str(&e.to_string()))?;
        }
        Ok(JsValue::UNDEFINED)
    });
    f.call1(raw, &cb).map(|_| ()).map_err(|e| format!("{e:?}"))
}

/// Rows one at a time: `rows` copies every row of a result into wasm memory first, and a log of 1 MB inline packs read
/// that way ran the isolate out of memory in `load` (F45, `tests/m1/results/m1-f43.md`).
fn each_row(
    sql: &SqlStorage,
    q: &str,
    args: Vec<SqlStorageValue>,
    mut f: impl FnMut(Vec<SqlStorageValue>) -> Result<bool>,
) -> Result<()> {
    for r in sql.exec(q, args)?.raw() {
        if !f(r?)? {
            break;
        }
    }
    Ok(())
}

/// The `request` table, read where it lives: in the member's SQLite, one row per point lookup. `GroupFold::apply`
/// writes a row here before `SqlStore` turns it into the input's `INSERT`, and drops rows past retention before the
/// input's `DELETE` runs, so until the transaction ran (`flushed`) reads see `pending` and hide rows older than
/// `cut`. Restore reads no row: holding every row in memory, as `GroupFold`'s in-memory table does, put a member with
/// 90,048 rows at 174.8 MB of wasm memory after restore, past its 128 MB isolate, and every request that did not
/// answer in its first poll died with the isolate (`tests/m2/results/m2-wp1.md`, "The restore defect").
pub struct RequestSql<E> {
    inner: Rc<RefCell<ReqInner<E>>>,
}

struct ReqInner<E> {
    sql: SqlStorage,
    pending: BTreeMap<RequestId, raft::RequestRow<E>>,
    cut: Timestamp,
    /// `Fold::row_event`: rows stored before it hold whole events, and a read keeps the row's form.
    row_event: fn(&E) -> E,
}

impl<E> Clone for RequestSql<E> {
    fn clone(&self) -> Self {
        RequestSql { inner: self.inner.clone() }
    }
}

impl<E> std::fmt::Debug for RequestSql<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RequestSql({} pending)", self.inner.borrow().pending.len())
    }
}

impl<E: raft::Record + Clone + std::fmt::Debug + serde::de::DeserializeOwned + 'static> RequestSql<E> {
    pub fn new(sql: SqlStorage, row_event: fn(&E) -> E) -> RequestSql<E> {
        RequestSql {
            inner: Rc::new(RefCell::new(ReqInner { sql, pending: BTreeMap::new(), cut: Timestamp(0), row_event })),
        }
    }

    /// The input's transaction ran: every pending row is in the table, and every expired one gone.
    pub fn flushed(&self) {
        self.inner.borrow_mut().pending.clear();
    }

    fn decode(&self, rid: RequestId, r: &[SqlStorageValue]) -> Result<raft::RequestRow<E>> {
        let i = self.inner.borrow();
        let events: Vec<E> = codec::from_bytes(blob(&r[2])?).map_err(|e| err(format!("request row {rid}: {e}")))?;
        Ok(raft::RequestRow {
            index: Index(int(&r[0])? as u64),
            at: Timestamp(int(&r[1])? as u64),
            events: events.iter().map(i.row_event).collect(),
        })
    }
}

impl<E: raft::Record + Clone + std::fmt::Debug + serde::de::DeserializeOwned + 'static> raft::RequestStore<E>
    for RequestSql<E>
{
    fn get(&self, id: RequestId) -> Option<raft::RequestRow<E>> {
        let (sql, cut) = {
            let i = self.inner.borrow();
            if let Some(r) = i.pending.get(&id) {
                return Some(r.clone());
            }
            (i.sql.clone(), i.cut)
        };
        let r = rows(&sql, "SELECT idx, at, events FROM request WHERE rid = ?", vec![id.to_string().into()])
            .expect("a request row read");
        let row = self.decode(id, r.first()?).expect("a request row decodes");
        (row.at >= cut).then_some(row)
    }

    fn put(&mut self, id: RequestId, row: raft::RequestRow<E>) -> bool {
        if self.get(id).is_some() {
            return false;
        }
        self.inner.borrow_mut().pending.insert(id, row);
        true
    }

    fn expire(&mut self, cut: Timestamp) {
        let mut i = self.inner.borrow_mut();
        i.cut = i.cut.max(cut);
        let c = i.cut;
        i.pending.retain(|_, r| r.at >= c);
    }

    fn rows(&self) -> Vec<(RequestId, raft::RequestRow<E>)> {
        let (sql, cut) = {
            let i = self.inner.borrow();
            (i.sql.clone(), i.cut)
        };
        let mut out = BTreeMap::new();
        each_row(&sql, "SELECT rid, idx, at, events FROM request", vec![], |r| {
            let rid = rid_from(text(&r[0])?)?;
            let row = self.decode(rid, &r[1..])?;
            if row.at >= cut {
                out.insert(rid, row);
            }
            Ok(true)
        })
        .expect("request rows read");
        out.extend(self.inner.borrow().pending.iter().map(|(k, v)| (*k, v.clone())));
        out.into_iter().collect()
    }

    fn clone_box(&self) -> Box<dyn raft::RequestStore<E>> {
        Box::new(self.clone())
    }
}

/// What `load` reads back: the crate's `Stored`, and the handle on its `request` table that `flush` tells.
type Loaded<F> = (Stored<<F as Family>::Fold>, RequestSql<<F as Family>::Ev>);

/// The base `load` restores above: every entry above `applied` is loaded, and below it the newest applied entries that
/// fit both `LOG_WINDOW` entries and `LOG_BYTES` bytes, at least the last applied one, and never below `floor` (a
/// truncated log's first row, which stays as its base).
fn load_base(sql: &SqlStorage, applied: Index, floor: u64) -> Result<u64> {
    let mut base = applied.0.saturating_sub(raft::LOG_WINDOW).max(floor);
    let mut bytes = 0u64;
    each_row(
        sql,
        "SELECT idx, length(row) FROM log WHERE idx > ? AND idx <= ? ORDER BY idx DESC",
        vec![i(base), i(applied.0)],
        |r| {
            let (idx, len) = (int(&r[0])? as u64, int(&r[1])? as u64);
            bytes += len;
            if bytes > raft::LOG_BYTES as u64 && idx < applied.0 {
                base = idx;
                return Ok(false);
            }
            Ok(true)
        },
    )?;
    Ok(base)
}

/// Reads the member's rows back into the crate's `Stored`: the state row, the log above a base below `applied`
/// (`load_base`: `LOG_WINDOW` entries and `LOG_BYTES` bytes at most), checked against its digests, and the
/// `GroupFold` loaded from `reg`, `request`, and `group_config` with `GroupFold::from_parts`. Entries at or below the
/// base stay on storage; the member asks for them with `Out::catchup`. While the log is short, the loaded fold is also
/// compared with the log replayed through `applied`. Every multi-row read goes one row at a time.
fn load<F: Family>(storage: &worker::Storage, join: &Join, retention: Duration) -> Result<Loaded<F>> {
    let sql = &storage.sql();
    F::prepare(sql)?;
    let state = match rows(sql, "SELECT row FROM raft_state WHERE id = 1", vec![])?.first() {
        Some(r) => {
            Some(codec::from_bytes::<raft::MemberState>(blob(&r[0])?).map_err(|e| err(format!("raft_state: {e}")))?)
        }
        None => None,
    };
    let applied = state.as_ref().map_or(Index(0), |s| s.applied);
    let decode = |r: &[SqlStorageValue]| -> Result<Entry<F>> {
        let idx = int(&r[0])? as u64;
        let e: Entry<F> = codec::from_bytes(blob(&r[2])?).map_err(|e| err(format!("log row {idx}: {e}")))?;
        if e.index.0 != idx {
            return Err(err(format!("log row {idx} holds index {:?}", e.index)));
        }
        if raft::digest(&e).0[..] != *blob(&r[1])? {
            return Err(err(format!("log row {idx} does not match its stored digest")));
        }
        Ok(e)
    };
    // A truncated log (`Host::truncate`) starts at its base row; restore starts there at the earliest. An installed
    // snapshot's base (`Write::Install`) has no row: `log_base` holds its term and digest.
    let installed = match rows(sql, "SELECT idx, term, digest FROM log_base WHERE id = 1", vec![])?.first() {
        Some(r) => Some(raft::LogBase {
            index: Index(int(&r[0])? as u64),
            term: raft::Term(int(&r[1])? as u64),
            digest: raft::Digest(blob(&r[2])?.try_into().map_err(|_| err("log_base digest"))?),
        }),
        None => None,
    };
    let first_row = rows(sql, "SELECT min(idx) FROM log", vec![])?.first().and_then(|r| int(&r[0]).ok()).unwrap_or(1);
    let floor = (first_row as u64).max(installed.map_or(0, |b| b.index.0));
    let truncated = floor > 1;
    let base_index = load_base(sql, applied, if truncated { floor } else { 0 })?;
    let mut base = raft::LogBase::default();
    if let Some(b) = installed.filter(|b| b.index.0 == base_index) {
        base = b;
    } else if base_index > 0 {
        let r = rows(sql, "SELECT idx, digest, row FROM log WHERE idx = ?", vec![i(base_index)])?;
        let e = decode(r.first().ok_or_else(|| err(format!("no log row at the base {base_index}")))?)?;
        base = raft::LogBase { index: e.index, term: e.term, digest: raft::digest(&e) };
    }
    let mut log: Vec<Entry<F>> = Vec::new();
    let mut prev = base.digest;
    each_row(sql, "SELECT idx, digest, row FROM log WHERE idx > ? ORDER BY idx", vec![i(base_index)], |r| {
        let e = decode(&r)?;
        if e.index.0 != base_index + log.len() as u64 + 1 || e.prev != prev {
            return Err(err(format!("log row {:?} does not chain after {} rows", e.index, log.len())));
        }
        prev = raft::digest(&e);
        log.push(e);
        Ok(true)
    })?;
    if applied.0 > base_index + log.len() as u64 {
        return Err(err(format!("applied {applied:?} past the log's {} rows", base_index + log.len() as u64)));
    }
    let folded = F::load_fold(sql, join)?;
    F::catch_up(storage, &folded, applied)?;
    let id_block = match rows(sql, "SELECT row FROM id_block WHERE id = 1", vec![])?.first() {
        Some(r) => Some(codec::from_bytes(blob(&r[0])?).map_err(|e| err(format!("id_block: {e}")))?),
        None => None,
    };
    let config = match rows(sql, "SELECT idx, row FROM group_config WHERE id = 1", vec![])?.first() {
        Some(r) => Some((
            Index(int(&r[0])? as u64),
            codec::from_bytes(blob(&r[1])?).map_err(|e| err(format!("group_config: {e}")))?,
        )),
        None => None,
    };
    let requests = RequestSql::<F::Ev>::new(sql.clone(), <F::Fold as raft::Fold>::row_event);
    let mut fold = GroupFold::from_parts(folded, applied, config, id_block, Box::new(requests.clone()));
    fold.set_retention(retention);
    if applied.0 <= RESTORE_CHECK_MAX && !truncated && !F::tiered() {
        let mut replayed = GroupFold::new(F::new_fold(sql, join));
        replayed.set_retention(retention);
        each_row(sql, "SELECT idx, digest, row FROM log WHERE idx <= ? ORDER BY idx", vec![i(applied.0)], |r| {
            replayed.apply(&decode(&r)?);
            Ok(true)
        })?;
        if replayed.fold != fold.fold {
            return Err(err(format!("the fold table differs from the log applied through {applied:?}")));
        }
        if replayed.config() != fold.config() || replayed.id_block() != fold.id_block() {
            return Err(err(format!("group_config differs from the log applied through {applied:?}")));
        }
        if replayed.requests() != fold.requests() {
            return Err(err(format!("request rows differ from the log applied through {applied:?}")));
        }
    }
    Ok((Stored { state, base, log, fold }, requests))
}

/// Stored entries `from..=through` (from 1 when `from` is 0), for `Member::catchup`, read one row at a time. Past the
/// anchor at `from` and the first entry after it, it stops at `APPEND_BYTES_MAX`, as `catchup` sends no more. `None`
/// if a row is missing, as it is while its transaction waits.
/// One stored log entry, if the log still holds it.
pub fn stored_entry<F: Family>(sql: &SqlStorage, at: Index) -> Option<Entry<F>> {
    let r = rows(sql, "SELECT row FROM log WHERE idx = ?", vec![i(at.0)]).ok()?;
    codec::from_bytes(blob(&r.first()?[0]).ok()?).ok()
}

fn stored_range<F: Family>(sql: &SqlStorage, from: Index, through: Index) -> Option<Vec<Entry<F>>> {
    let lo = from.0.max(1);
    let mut entries: Vec<Entry<F>> = Vec::new();
    let (mut bytes, mut whole) = (0usize, true);
    let q = "SELECT idx, length(row), row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx";
    each_row(sql, q, vec![i(lo), i(through.0)], |r| {
        let idx = int(&r[0])? as u64;
        if idx != lo + entries.len() as u64 {
            whole = false;
            return Ok(false);
        }
        if idx > from.0 {
            bytes += int(&r[1])? as usize;
            if bytes > raft::APPEND_BYTES_MAX && idx > from.0 + 1 {
                return Ok(false);
            }
        }
        match codec::from_bytes(blob(&r[2])?) {
            Ok(e) => entries.push(e),
            Err(_) => whole = false,
        }
        Ok(whole)
    })
    .ok()?;
    let reached = (lo + entries.len() as u64).saturating_sub(1);
    (whole && !entries.is_empty() && (reached == through.0 || bytes > raft::APPEND_BYTES_MAX)).then_some(entries)
}

#[cfg(feature = "test-faults")]
fn outcome_index<F: Family>(o: &Outcome<F>) -> Index {
    match o {
        Ok(a) => a.index,
        Err(CommitError::Refused { index, .. }) => *index,
        Err(CommitError::NotLeader(_) | CommitError::Full { .. }) => Index(0),
    }
}

impl<F: Family> Host<F> {
    pub fn new(state: Rc<State>, env: Env) -> Rc<Host<F>> {
        let sql = state.storage().sql();
        for q in SCHEMA.iter().chain(F::schema()) {
            sql.exec(q, None).expect("member schema");
        }
        let h = Rc::new(Host {
            state,
            env,
            core: RefCell::new(None),
            requests: RefCell::new(None),
            restore_error: RefCell::new(None),
            dying: Cell::new(false),
            ticking: Cell::new(false),
            active_until: Cell::new(0),
            alarm_at: Cell::new(0),
            served_at: Cell::new(0),
            #[cfg(feature = "test-faults")]
            down_until: Cell::new(0),
            queue: RefCell::new(Vec::new()),
            flush_due: Cell::new(false),
            held: RefCell::new(Vec::new()),
            flush_waiters: RefCell::new(Vec::new()),
            unconfirmed: RefCell::new(Vec::new()),
            stats: RefCell::new(Stats::default()),
            synced: RefCell::new(VecDeque::new()),
            queued_last: Cell::new(None),
            truncs: RefCell::new(VecDeque::new()),
            blocked: RefCell::new(Vec::new()),
            leasing: Cell::new(false),
            callers: RefCell::new(Default::default()),
            #[cfg(feature = "test-faults")]
            abandon: RefCell::new(BTreeSet::new()),
            #[cfg(feature = "test-faults")]
            hop: Cell::new(2),
        });
        #[cfg(feature = "test-faults")]
        for q in crate::faults::SCHEMA {
            h.state.storage().sql().exec(q, None).expect("fault schema");
        }
        #[cfg(feature = "test-faults")]
        if let Ok(r) = rows(&h.state.storage().sql(), "SELECT v FROM me WHERE k = 'down_until'", vec![])
            && let Some(v) = r.first().and_then(|r| text(&r[0]).ok()).and_then(|v| v.parse().ok())
        {
            h.down_until.set(v);
        }
        if let Err(e) = h.restore() {
            console_error!("member restore failed: {e}");
            *h.restore_error.borrow_mut() = Some(e.to_string());
        }
        h
    }

    /// Test installs: one `Stats::trace` entry, timed now.
    #[cfg(feature = "test-faults")]
    pub fn trace(&self, kind: Trace, peer: u8, batch: u64, index: u64, ms: u64) {
        let mut s = self.stats.borrow_mut();
        if s.trace.len() >= TRACE_MAX {
            s.trace.pop_front();
        }
        s.trace.push_back([now_ms(), kind as u64, u64::from(peer), batch, index, ms]);
    }

    #[cfg(not(feature = "test-faults"))]
    pub fn trace(&self, _: Trace, _: u8, _: u64, _: u64, _: u64) {}

    /// Test installs: traces a `ReadIndexRes` stepped here, `own` when it answered this member's own call.
    pub fn trace_res<E>(&self, e: &raft::Envelope<E>, own: bool) {
        if let raft::Message::ReadIndexRes { batch, index } = &e.msg {
            self.trace(Trace::ResStepped, e.from.0, *batch, index.map_or(0, |(i, _)| i.0), u64::from(!own));
        }
    }

    /// The ticket a re-sent `NeedIdBlock` command now waits on.
    pub fn current(&self, t: Ticket) -> Ticket {
        let core = self.core.borrow();
        let mut t = t;
        if let Some(c) = core.as_ref() {
            while let Some(n) = c.reissued.get(&t) {
                t = *n;
            }
        }
        t
    }

    fn stored_join(&self) -> Result<Option<Join>> {
        let rows = rows(&self.state.storage().sql(), "SELECT v FROM me WHERE k = 'join'", vec![])?;
        match rows.first() {
            Some(r) => Ok(Some(serde_json::from_str(text(&r[0])?)?)),
            None => Ok(None),
        }
    }

    /// Builds the core from storage, once the member knows who it is (`/join` stored it).
    pub fn restore(self: &Rc<Self>) -> Result<()> {
        let Some(join) = self.stored_join()? else { return Ok(()) };
        let site = SiteEnv::read(&self.env)?;
        let retention = request_retention(&self.env);
        let (stored, requests) = load::<F>(&self.state.storage(), &join, retention)?;
        *self.requests.borrow_mut() = Some(requests);
        let applied = stored.state.as_ref().map_or(Index(0), |s| s.applied);
        // Everything `load` read back is durable.
        *self.synced.borrow_mut() = VecDeque::from([(stored.base.index.0 + stored.log.len() as u64, now_ms())]);
        let (mut m, ready) =
            raft::Member::<F::C>::restore(member_config::<F>(&site, &join), stored, Timestamp(now_ms()), seed())
                .map_err(|e| err(e.to_string()))?;
        m.set_request_retention(retention);
        m.set_log_window_bytes(raft::LOG_WINDOW, raft::LOG_BYTES);
        let mut store = SqlStore::<F> {
            m: &m,
            fold: &m.group_fold().fold,
            applied,
            stmts: Vec::new(),
            archived: Vec::new(),
            installed: None,
        };
        let ((), _out) = ready.persist(&mut store);
        let (applied, stmts) = (store.applied, store.stmts);
        self.queue.borrow_mut().extend(stmts);
        self.flush().map_err(err)?;
        *self.core.borrow_mut() = Some(Core {
            m,
            join,
            applied,
            waiters: HashMap::new(),
            admitted: HashMap::new(),
            reads: HashMap::new(),
            #[cfg(feature = "test-faults")]
            kills: HashMap::new(),
            reissued: HashMap::new(),
            blocks: Vec::new(),
            recorded: Vec::new(),
            #[cfg(feature = "test-faults")]
            no_marker: std::collections::HashSet::new(),
            #[cfg(feature = "test-faults")]
            hold_deps_until: 0,
        });
        self.stats.borrow_mut().restores += 1;
        F::restored(self);
        Ok(())
    }

    /// Inside an injected `down_ms` window after a kill: the object answers nothing but `/status`.
    #[cfg(feature = "test-faults")]
    pub fn down(&self) -> bool {
        let until = self.down_until.get();
        until != 0 && now_ms() < until
    }

    pub fn joined(&self) -> bool {
        self.core.borrow().is_some()
    }

    fn note(&self, core: &Core<F>, writes: &[Write<F::Ev>], t: u64) {
        let mut s = self.stats.borrow_mut();
        let (leading, term) = (core.m.role() == Role::Leader, core.m.term());
        for w in writes {
            match w {
                Write::Append(e) if leading && e.term == term && matches!(e.body, EntryBody::Events(_)) => {
                    s.entries += 1;
                    s.commands += e.requests.len() as u32;
                    s.appended.push((e.index.0, t));
                }
                Write::Apply(to) => {
                    for idx in core.applied.0 + 1..=to.0 {
                        if let Some(e) = core.m.entry_at(Index(idx))
                            && s.apply_ms.len() < SAMPLES_MAX
                        {
                            s.apply_ms.push(t.saturating_sub(e.at.0) as u32);
                        }
                    }
                    let done: Vec<u64> = s.appended.iter().filter(|(x, _)| *x <= to.0).map(|(_, at)| *at).collect();
                    s.appended.retain(|(x, _)| *x > to.0);
                    for at in done {
                        if s.round_ms.len() < SAMPLES_MAX {
                            s.round_ms.push(t.saturating_sub(at) as u32);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// One input to the member: run it, make its writes durable, and return its reply and outputs, not yet released
    /// (`dispatch` releases them). The kill faults fire here.
    pub fn step<R>(
        self: &Rc<Self>,
        f: impl FnOnce(&mut raft::Member<F::C>, Timestamp) -> Ready<F::C, R>,
    ) -> Result<(R, Out<F::C>)> {
        if self.dying.get() {
            return Err(err("member is resetting"));
        }
        self.kick();
        let mut guard = self.core.try_borrow_mut().map_err(|_| err("member core busy"))?;
        let core = guard.as_mut().ok_or_else(|| err("member not joined"))?;
        let t = now_ms();
        let applied0 = core.applied;
        let ready = f(&mut core.m, Timestamp(t));
        self.note(core, &ready.writes, t);
        #[cfg(feature = "test-faults")]
        let appended = self.appended_kill(core, &ready.writes);
        let wrote = !ready.writes.is_empty();
        for w in &ready.writes {
            match w {
                Write::Append(e) => self.queued_last.set(Some(e.index.0)),
                Write::Truncate(x) => {
                    let cap = x.0 - 1;
                    self.queued_last.set(self.queued_last.get().map(|l| l.min(cap)));
                    for r in self.synced.borrow_mut().iter_mut() {
                        r.0 = r.0.min(cap);
                    }
                    let mut t = self.truncs.borrow_mut();
                    let n = t.back().map_or(0, |t| t.0) + 1;
                    t.push_back((n, cap));
                    if t.len() > SYNCED_KEEP {
                        t.pop_front();
                    }
                }
                _ => {}
            }
        }
        let mut store = SqlStore::<F> {
            m: &core.m,
            fold: &core.m.group_fold().fold,
            applied: core.applied,
            stmts: vec![],
            archived: vec![],
            installed: None,
        };
        let (r, mut out) = ready.persist(&mut store);
        let (applied, stmts, archived) = (store.applied, store.stmts, store.archived);
        if let Some(b) = store.installed {
            self.queued_last.set(Some(b.index.0));
            *self.synced.borrow_mut() = VecDeque::new();
        }
        // A fold read storage refused during `decide` or `apply`: nothing this input computed may leave.
        if let Some(why) = F::lost(&core.m.group_fold().fold) {
            drop(guard);
            self.fail_lost(&why);
            return Err(err(format!("fold storage lost: {why}")));
        }
        if applied > applied0 {
            self.trace(Trace::Applied, 0, 0, applied.0, 0);
            let events: usize = (applied0.0 + 1..=applied.0)
                .filter_map(|x| core.m.entry_at(Index(x)))
                .map(|e| match &e.body {
                    EntryBody::Events(evs) => evs.len(),
                    _ => 0,
                })
                .sum();
            let mut s = self.stats.borrow_mut();
            if s.apply_steps.len() < SAMPLES_MAX {
                let ms = now_ms().saturating_sub(t) as u32;
                s.apply_steps.push((ms, (applied.0 - applied0.0) as u32, events as u32));
            }
        }
        if let Some(ms) = F::fold_alarm_ms(core.m.role() == Role::Leader)
            && applied > core.applied
        {
            self.stats.borrow_mut().alarm_arms += 1;
            self.arm_in(ms);
        }
        #[cfg(feature = "test-faults")]
        if let Some(fl) = Self::fault_for(core, appended, &out) {
            core.kills.remove(&fl.rid);
            drop(guard);
            return Err(self.fire(fl, out, stmts));
        }
        core.applied = applied;
        let early = std::mem::take(&mut out.early);
        if wrote {
            let id = core.m.confirm_begin(Timestamp(now_ms()));
            let timed = core.m.role() == Role::Leader
                && early
                    .iter()
                    .chain(&out.messages)
                    .any(|e| matches!(&e.msg, raft::Message::Append(a) if !a.entries.is_empty()));
            self.unconfirmed.borrow_mut().push((id, timed.then(now_ms), out.self_ack.take()));
        }
        drop(guard);
        self.queue.borrow_mut().extend(stmts);
        let defer = !early.is_empty() && wrote;
        // The leader's AppendEntries leave now. Each `send` issues its fetch in a microtask, before the deferred
        // transaction's macrotask, so the output gate does not hold the round behind the leader's own write (Raft
        // dissertation 10.2.1, and Milestone 0's prototype). The leader counts its own copy only once its `SelfAck`
        // comes back after `sync()`.
        for e in early {
            self.send(e);
        }
        if defer || self.flush_due.get() {
            self.defer_flush();
        } else if let Err(e) = self.flush() {
            self.fail();
            return Err(err(format!("storage write failed: {e}")));
        }
        for (key, to) in archived {
            let h = self.clone();
            spawn_local(async move { h.truncate(&key, to).await });
        }
        Ok((r, out))
    }

    /// After an `Archived` through `to` applied: once its key is in two buckets, the stored log keeps no entry below
    /// `to` (`design/DESIGN.md`, Shape, "Packs": "followers truncate only through a committed range whose key they
    /// can HEAD in two buckets"). The row at `to` stays as the stored log's base: its term and digest anchor the next
    /// entry, and restore starts above it. Entries through `to` are archived either way (`RepoFold::first_index`).
    /// A key in fewer than two buckets truncates nothing and raises an alarm; re-putting the copy is not built.
    async fn truncate(self: &Rc<Self>, key: &str, to: u64) {
        self.settled().await;
        let Ok(site) = SiteEnv::read(&self.env) else { return };
        let mut held = 0;
        for s in &site.sites.0 {
            if let Ok(b) = self.env.bucket(&format!("BUCKET_{}", s.id.0))
                && let Ok(Some(_)) = b.head(key).await
            {
                held += 1;
            }
        }
        if held < 2 {
            console_error!("archive {key} is in {held} buckets; the log is not truncated");
            self.stats.borrow_mut().alarms.push(format!("{} archive {key} in {held} buckets", now_ms()));
            return;
        }
        match self.state.storage().sql().exec("DELETE FROM log WHERE idx < ?", vec![i(to)]) {
            Ok(_) => self.stats.borrow_mut().truncated_to = to,
            Err(e) => return console_error!("truncate below {to}: {e}"),
        }
        F::truncated(self, Index(to)).await;
    }

    /// A command marked `before_commit*` whose entry this input appends.
    #[cfg(feature = "test-faults")]
    fn appended_kill(&self, core: &Core<F>, writes: &[Write<F::Ev>]) -> Option<(RequestId, Kill, u64, Entry<F>)> {
        writes.iter().find_map(|w| match w {
            Write::Append(e) => e.requests.iter().find_map(|(rid, _)| match core.kills.get(rid) {
                Some((k, down)) if k.at_append() => Some((*rid, *k, *down, e.clone())),
                _ => None,
            }),
            _ => None,
        })
    }

    /// The kill this input reaches: a marked command whose entry it appends, or whose ticket it resolves.
    #[cfg(feature = "test-faults")]
    fn fault_for(core: &Core<F>, appended: Option<(RequestId, Kill, u64, Entry<F>)>, out: &Out<F::C>) -> Option<Fault> {
        if let Some((rid, kill, down_ms, e)) = appended {
            // The crate names the admitted commands this entry's `decide` refused.
            let carried: BTreeSet<RequestId> = e.requests.iter().map(|(r, _)| *r).collect();
            let refused: Vec<String> =
                out.refused.iter().filter(|(i, _)| *i == e.index).map(|(_, r)| r.to_string()).collect();
            Some(Fault {
                rid,
                kill,
                down_ms,
                index: e.index,
                term: e.term,
                result: json!({
                    "requests": carried.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                    "refusedInEntry": refused,
                    "digest": digest_hex(&raft::digest(&e)),
                }),
            })
        } else {
            out.resolutions.iter().find_map(|r| match r {
                Resolution::Ticket { ticket, result } if !matches!(result, Err(CommitError::NotLeader(_))) => {
                    let (rid, _, _) = core.admitted.get(ticket)?;
                    match core.kills.get(rid) {
                        Some((k @ (Kill::AfterQuorum | Kill::AfterQuorumUnsynced), down)) => Some(Fault {
                            rid: *rid,
                            kill: *k,
                            down_ms: *down,
                            index: outcome_index::<F>(result),
                            term: core.m.term(),
                            result: outcome_json::<F>(&core.m.group_fold().fold, result),
                        }),
                        _ => None,
                    }
                }
                _ => None,
            })
        }
    }

    /// Fires a kill: make the writes it keeps durable, release what it lets leave, record it, and reset the object.
    #[cfg(feature = "test-faults")]
    fn fire(self: &Rc<Self>, fl: Fault, mut out: Out<F::C>, stmts: Vec<Stmt>) -> Error {
        // Earlier inputs' queued writes run first, as they would have; this input's only for the kills that keep
        // them.
        if fl.kill.keeps_writes() {
            self.queue.borrow_mut().extend(stmts);
        }
        if let Err(e) = self.flush() {
            self.fail();
            return err(format!("storage write failed: {e}"));
        }
        let delay = match fl.kill {
            Kill::BeforeCommitSent => {
                // The entry and its messages leave once durable; the leader dies before it hears an answer.
                self.dying.set(true);
                self.release(out, None);
                50
            }
            Kill::BeforeCommitEarly => {
                // Only the AppendEntries leave, before the write, which is never made.
                self.dying.set(true);
                for e in std::mem::take(&mut out.early) {
                    self.send(e);
                }
                50
            }
            _ => 0,
        };
        self.die(Some(fl), Duration::from_millis(delay));
        err("killed by an injected fault")
    }

    /// Runs the queued transaction one macrotask from now. Until it has run, every output is held (`dispatch`), and
    /// every response waits for it (`settled`).
    fn defer_flush(self: &Rc<Self>) {
        if self.flush_due.replace(true) {
            return;
        }
        let h = self.clone();
        spawn_local(async move {
            Delay::from(Duration::ZERO).await;
            if h.dying.get() {
                return;
            }
            if let Err(e) = h.flush() {
                console_error!("deferred storage write failed: {e}");
                h.fail();
            }
        });
    }

    /// Resolves once no transaction is deferred, so a response sent after it is held by the output gate until the
    /// writes it may depend on are durable.
    pub async fn settled(&self) {
        if !self.flush_due.get() {
            return;
        }
        let (tx, rx) = oneshot::channel();
        self.flush_waiters.borrow_mut().push(tx);
        let _ = rx.await;
    }

    /// Runs every queued statement in one transaction, releases the outputs that waited for it, and reports the
    /// storage confirm, and each `SelfAck`, once `sync()` says the writes are durable. The Durable Object confirms
    /// writes in order, so one `sync()` covers every input queued so far.
    fn flush(self: &Rc<Self>) -> std::result::Result<(), String> {
        let stmts = std::mem::take(&mut *self.queue.borrow_mut());
        commit_tx(&self.state.storage(), stmts)?;
        if let Some(r) = self.requests.borrow().as_ref() {
            r.flushed();
        }
        if let Ok(core) = self.core.try_borrow()
            && let Some(c) = core.as_ref()
        {
            F::flushed(&c.m.group_fold().fold);
        }
        self.flush_due.set(false);
        let held = std::mem::take(&mut *self.held.borrow_mut());
        for out in held {
            self.release(out, None);
        }
        for tx in std::mem::take(&mut *self.flush_waiters.borrow_mut()) {
            let _ = tx.send(());
        }
        let pending = std::mem::take(&mut *self.unconfirmed.borrow_mut());
        let last = self.queued_last.take();
        let truncs = self.truncs.borrow().back().map_or(0, |t| t.0);
        if !pending.is_empty() {
            let h = self.clone();
            spawn_local(async move {
                if h.state.storage().sync().await.is_err() || h.dying.get() {
                    return;
                }
                if let Some(l) = last {
                    // A truncation stepped after this transaction ran, before its confirm, caps what it vouches for.
                    let l = h.truncs.borrow().iter().filter(|(n, _)| *n > truncs).map(|(_, c)| *c).fold(l, u64::min);
                    let mut synced = h.synced.borrow_mut();
                    synced.push_back((l, now_ms()));
                    while synced.len() > SYNCED_KEEP {
                        synced.pop_front();
                    }
                }
                for (id, timed, ack) in pending {
                    if let Some(t0) = timed {
                        let mut s = h.stats.borrow_mut();
                        if s.confirm_ms.len() < SAMPLES_MAX {
                            s.confirm_ms.push(now_ms().saturating_sub(t0) as u32);
                        }
                    }
                    let _ = h.input(|m, t| m.confirm_end(t, id));
                    if let Some(ack) = ack {
                        let _ = h.input(|m, t| m.self_ack(t, ack));
                    }
                }
            });
        }
        Ok(())
    }

    /// Releases what a persisted input produced: ticket and read resolutions to their waiters, messages to peers.
    /// Envelopes addressed to `reply_to` are returned instead of sent, for the response to the call that carried
    /// the input.
    ///
    /// While a transaction is deferred, everything is held for it but the envelopes for `reply_to`, which go into a
    /// response that waits for it (`settled`).
    pub fn dispatch(self: &Rc<Self>, mut out: Out<F::C>, reply_to: Option<SiteId>) -> Vec<Envelope<F>> {
        if !self.flush_due.get() {
            return self.release(out, reply_to);
        }
        let (back, rest): (Vec<Envelope<F>>, Vec<Envelope<F>>) =
            std::mem::take(&mut out.messages).into_iter().partition(|e| Some(e.to) == reply_to);
        out.messages = rest;
        self.held.borrow_mut().push(out);
        back
    }

    fn release(self: &Rc<Self>, out: Out<F::C>, reply_to: Option<SiteId>) -> Vec<Envelope<F>> {
        let mut back = Vec::new();
        let mut send = Vec::new();
        let mut need_block = false;
        let mut marks = Vec::new();
        let deps = out.deps;
        let catchup = out.catchup;
        let installs = out.install;
        let mut lost = None;
        {
            let Ok(mut guard) = self.core.try_borrow_mut() else { return back };
            let Some(core) = guard.as_mut() else { return back };
            let now = now_ms();
            for r in out.resolutions {
                match r {
                    Resolution::Ticket { ticket, result } => {
                        let admitted = core.admitted.remove(&ticket);
                        #[cfg(feature = "test-faults")]
                        if let Some((rid, _, _)) = &admitted {
                            // A kill rides on one admission only: a ticket that ended `NotLeader` drops it.
                            core.kills.remove(rid);
                        }
                        // A command refused only for want of an id block is not answered: the lease loop records a
                        // block and re-sends it under the same request id, which a refusal left no row for.
                        if let (Err(CommitError::Refused { err: e, .. }), Some((rid, _, Some(cmd)))) =
                            (&result, &admitted)
                            && F::needs_block(e)
                        {
                            self.blocked.borrow_mut().push((ticket, *rid, cmd.clone()));
                            need_block = true;
                            continue;
                        }
                        if let Some((_, at, _)) = admitted
                            && !matches!(result, Err(CommitError::NotLeader(_)))
                        {
                            let mut s = self.stats.borrow_mut();
                            if s.commit_ms.len() < SAMPLES_MAX {
                                s.commit_ms.push(now.saturating_sub(at) as u32);
                            }
                        }
                        // The commit's work outside the log, on the leader that decided it: a repository's pack
                        // markers. Marker puts are create-only, so a re-sent command marks nothing twice.
                        if let (Ok(a), Some((rid, _, Some(cmd)))) = (&result, &admitted) {
                            #[cfg(feature = "test-faults")]
                            let skip = core.no_marker.remove(rid);
                            #[cfg(not(feature = "test-faults"))]
                            let skip = {
                                let _ = rid;
                                false
                            };
                            if !skip {
                                marks.push((cmd.clone(), a.clone()));
                            }
                        }
                        for tx in core.waiters.remove(&ticket).unwrap_or_default() {
                            let _ = tx.send(result.clone());
                        }
                    }
                    Resolution::Read { id, result } => {
                        if let Some((q, tx)) = core.reads.remove(&id) {
                            // `fresh` only before the member's next input: this runs right after the input. A `dep`
                            // read answers the whole fold in the crate's codec, hex, for another group's `deps`.
                            let v = result.and_then(|g| {
                                core.m.fresh(&g).map(|f| {
                                    // `probe`: only that a member leads (`graftyard_lookup::create`).
                                    let body = if q.get("probe").is_some() {
                                        Value::Null
                                    } else if q.get("dep").is_some() {
                                        json!(hex(&F::fold_bytes(f.fold())))
                                    } else if let Some(rid) = q.get("request").and_then(Value::as_str) {
                                        // The sweeper's `RequestSource`: the row at the same applied index.
                                        match rid.parse() {
                                            Ok(rid) => core.m.group_fold().request(rid).map_or(Value::Null, |row| {
                                                json!({ "index": row.index.0, "packs": F::row_packs(&row.events) })
                                            }),
                                            Err(_) => json!({ "badRequestId": rid }),
                                        }
                                    } else {
                                        F::read_json(f.fold(), &q)
                                    };
                                    (body, f.index())
                                })
                            });
                            // An answer read while storage refused a row is not sent: the caller sees a reset.
                            if let Some(why) = F::lost(&core.m.group_fold().fold) {
                                lost = Some(why);
                                continue;
                            }
                            let _ = tx.send(v);
                        }
                    }
                    Resolution::IdBlock { block, result } => {
                        let (mine, rest): (Vec<_>, Vec<_>) =
                            std::mem::take(&mut core.blocks).into_iter().partition(|(b, _)| *b == block);
                        core.blocks = rest;
                        for (_, tx) in mine {
                            let _ = tx.send(result.clone());
                        }
                    }
                    Resolution::Recorded { id, result } => {
                        let (mine, rest): (Vec<_>, Vec<_>) =
                            std::mem::take(&mut core.recorded).into_iter().partition(|(r, _)| *r == id);
                        core.recorded = rest;
                        for (_, tx) in mine {
                            let _ = tx.send(result.clone());
                        }
                    }
                    Resolution::Campaign(_) => {}
                }
            }
            if !out.alarms.is_empty() {
                let mut s = self.stats.borrow_mut();
                for a in out.alarms {
                    console_log!("member alarm: {a:?}");
                    if s.alarms.len() < 50 {
                        s.alarms.push(format!("{now} {a:?}"));
                    }
                }
            }
            for e in out.early.into_iter().chain(out.messages) {
                if Some(e.to) == reply_to { back.push(e) } else { send.push(e) }
            }
        }
        if let Some(why) = lost {
            self.fail_lost(&why);
            return Vec::new();
        }
        for e in send {
            self.send(e);
        }
        // A follower behind the window: its entries come from the `log` table. Every append this member stored is
        // durable by now (`release` runs after the transaction), so a missing row means the request is stale.
        for c in catchup {
            if let Some(entries) = stored_range::<F>(&self.state.storage().sql(), c.from, c.through) {
                self.stats.borrow_mut().catchups += 1;
                let _ = self.input(|m, t| m.catchup(t, c, entries));
            }
        }
        for want in deps {
            self.read_deps(want);
        }
        for w in installs {
            F::install_wanted(self, w);
        }
        if !marks.is_empty()
            && let Some(chain) = self.core.borrow().as_ref().map(|c| c.join.chain)
        {
            for (cmd, a) in marks {
                let env = self.env.clone();
                let h = self.clone();
                spawn_local(async move {
                    let failed = F::after_ok(env, chain, cmd, a).await;
                    h.stats.borrow_mut().marker_failures += failed as u32;
                });
            }
        }
        if need_block {
            self.lease();
        }
        back
    }

    /// The member's `request` table handle, for a fold built over its tables (`crate::load`).
    pub fn request_store(&self) -> Option<RequestSql<F::Ev>> {
        self.requests.borrow().clone()
    }

    /// Steps and releases, for inputs whose reply needs nothing registered first.
    pub fn input<R>(
        self: &Rc<Self>,
        f: impl FnOnce(&mut raft::Member<F::C>, Timestamp) -> Ready<F::C, R>,
    ) -> Result<R> {
        let (r, out) = self.step(f)?;
        self.dispatch(out, None);
        Ok(r)
    }

    /// The site and object name of the member at raft address `to`: a voter's site, or a learner's slot address.
    fn peer_name(&self, to: SiteId) -> Option<(SiteId, String)> {
        let core = self.core.borrow();
        let join = &core.as_ref()?.join;
        // The latest config in the log names the members, as it counts their acks; until the group has one, the
        // bootstrap names from the record.
        if core.as_ref()?.m.committed_config().is_some() {
            let c = core.as_ref()?.m.latest_config();
            let names = match c {
                raft::GroupConfig::Normal(p) | raft::GroupConfig::Degraded { placement: p, .. } => vec![p],
                raft::GroupConfig::Joint { old, new } => vec![new, old],
            };
            for p in names {
                if let Some(m) = p.voters().iter().chain(p.learners()).find(|m| m.addr() == to) {
                    return Some((m.site, m.do_name.to_string()));
                }
            }
        }
        join.record
            .bootstrap_members(join.chain)
            .into_iter()
            .find(|m| m.addr() == to)
            .map(|m| (m.site, m.do_name.into()))
    }

    fn send(self: &Rc<Self>, e: Envelope<F>) {
        // An abandoned transfer's `TimeoutNow` never reaches the target, so the leader abandons it after the LEASE.
        #[cfg(feature = "test-faults")]
        if let raft::Message::TimeoutNow { transfer, .. } = &e.msg
            && self.abandon.borrow().contains(&transfer.0)
        {
            crate::faults::count("abandoned_timeout_now");
            return;
        }
        // A learner calling this member gets the message in the response to its next call, never as a call back.
        let Some(e) = self.callers.borrow_mut().route(e.to, e, now_ms()) else { return };
        self.call(e);
    }

    /// Sends `e` as a member-to-member call and steps the replies.
    fn call(self: &Rc<Self>, e: Envelope<F>) {
        let h = self.clone();
        spawn_local(async move {
            let body = codec::to_bytes(std::slice::from_ref(&e));
            let call = Call::of(&e, body.len());
            match (&e.msg, call.batch) {
                (raft::Message::ReadIndex { .. }, Some(b)) => h.trace(Trace::IndexSent, e.to.0, b, 0, 0),
                (raft::Message::ReadIndexRes { index, .. }, Some(b)) => {
                    h.trace(Trace::ResSent, e.to.0, b, index.map_or(0, |(i, _)| i.0), 1)
                }
                _ => {}
            }
            // What `call_failed` needs, so the envelope and its entries are not held while a late answer is awaited.
            let (to, on_miss, on_error) = (e.to, e.failure(true), e.failure(false));
            #[cfg(feature = "test-faults")]
            let drop_reply = matches!(e.msg, raft::Message::Append(_));
            drop(e);
            #[cfg(feature = "test-faults")]
            if crate::faults::site_down(&h.env).await {
                crate::faults::count(&format!("site_fault:out>{}", to.0));
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "site fault")));
                return h.call_failed(on_error);
            }
            #[cfg(feature = "test-faults")]
            if crate::faults::fail_call(&h.env, to.0) {
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "fail_call")));
                return h.call_failed(on_error);
            }
            #[cfg(feature = "test-faults")]
            call.hop.set(h.hop.get() + 1);
            let missed = || {
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Deadline, "no answer within CALL_DEADLINE")));
                h.stats.borrow_mut().failed_calls += 1;
                h.call_failed(on_miss);
            };
            // `answer` starts the limit's timer only once the deadline passed.
            let (late, r) = match answer(
                h.call_peer(to, body, call.hop.get()),
                Delay::from(raft::CALL_DEADLINE),
                Delay::from(LATE_LIMIT),
                missed,
            )
            .await
            {
                Answer::InTime(r) => (false, r),
                Answer::Late(r) => (true, r),
                Answer::Missed => return,
            };
            let replies = match r {
                Ok(replies) => replies,
                Err(pe) if late => return h.ended(&call, End::LateFailed(pe)),
                Err(pe) => {
                    h.ended(&call, End::Failed(pe));
                    h.stats.borrow_mut().failed_calls += 1;
                    return h.call_failed(on_error);
                }
            };
            // The callee ran the call and its answer is lost: the leader sees a call that missed its deadline.
            #[cfg(feature = "test-faults")]
            if drop_reply && crate::faults::drop_append_reply(&h.env, to.0) {
                if !late {
                    h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "drop_append_reply")));
                    h.stats.borrow_mut().failed_calls += 1;
                    h.call_failed(on_miss);
                }
                return;
            }
            h.ended(&call, if late { End::Late } else { End::Ok });
            for r in replies {
                let from = r.from;
                h.trace_res(&r, true);
                if h.input(|m, t| m.step(t, from, r.msg)).is_err() {
                    break;
                }
            }
        });
    }

    fn call_failed(self: &Rc<Self>, f: Option<raft::CallFailure>) {
        if let Some(f) = f {
            let _ = self.input(|m, t| m.call_failed(t, f));
        }
    }

    /// Records how a call ended in `/stats`: per peer, its counts, its last errors, and its last AppendEntries.
    fn ended(&self, c: &Call, end: End) {
        let now = now_ms();
        let ms = now.saturating_sub(c.at) as u32;
        let mut s = self.stats.borrow_mut();
        let appended = c.append.is_some() && c.entries > 0;
        if appended && matches!(end, End::Ok) {
            let v = s.append_ms.entry(c.to).or_default();
            if v.len() < SAMPLES_MAX {
                v.push(ms);
            }
        }
        let depth = matches!(&end, End::Failed(e) | End::LateFailed(e) if e.msg.contains("Subrequest depth"));
        if !matches!(&end, End::Failed(e) if e.kind == CallFail::Deadline) {
            let k = if matches!(end, End::Ok | End::Late) {
                0
            } else if depth {
                1
            } else {
                2
            };
            s.hops.entry(c.hop.get()).or_default()[k] += 1;
        }
        if depth {
            drop(s);
            self.trace(Trace::DepthFail, c.to, 0, c.hop.get(), u64::from(ms));
            s = self.stats.borrow_mut();
        }
        if let Some(b) = c.batch {
            let kind = if c.msg == "readIndex" { Trace::IndexEnded } else { Trace::ResEnded };
            let code = match &end {
                End::Ok => 0,
                End::Late => 1,
                End::Failed(_) => 2,
                End::LateFailed(_) => 3,
            };
            drop(s);
            self.trace(kind, c.to, b, code, u64::from(ms));
            s = self.stats.borrow_mut();
        }
        let ct = s.calls.entry(c.msg).or_default();
        let ok = matches!(end, End::Ok | End::Late);
        if ok {
            ct.ok += 1
        } else {
            ct.failed += 1
        }
        if ct.ms.len() < SAMPLES_MAX {
            ct.ms.push(ms);
        }
        if ms >= SLOW_CALL_MS || !ok {
            let why = match &end {
                End::Failed(e) | End::LateFailed(e) => format!("{}: {}", e.kind.name(), e.msg),
                End::Late => "late".into(),
                End::Ok => "ok".into(),
            };
            ct.slow.push_back(json!({ "at": c.at, "to": c.to, "ms": ms, "end": why }));
            if ct.slow.len() > PEER_APPENDS {
                ct.slow.pop_front();
            }
        }
        let p = s.peers.entry(c.to).or_default();
        let tag = match &end {
            End::Ok => {
                p.ok += 1;
                "ok"
            }
            End::Late => {
                p.late += 1;
                p.late_ms_max = p.late_ms_max.max(ms);
                "late"
            }
            End::Failed(e) | End::LateFailed(e) => {
                let late = matches!(end, End::LateFailed(_));
                let counts = if late { &mut p.late_failed } else { &mut p.failed };
                *counts.entry(e.kind.name()).or_insert(0) += 1;
                p.errors.push_back(json!({
                    "at": now, "kind": e.kind.name(), "msg": e.msg, "late": late,
                    "call": c.msg, "entries": c.entries, "bytes": c.bytes, "ms": ms,
                }));
                if p.errors.len() > PEER_ERRORS {
                    p.errors.pop_front();
                }
                e.kind.name()
            }
        };
        if let (true, Some((round, first, last))) = (appended, c.append) {
            p.appends.push_back(json!({
                "at": now, "round": round, "first": first, "last": last, "bytes": c.bytes, "ms": ms, "end": tag,
            }));
            if p.appends.len() > PEER_APPENDS {
                p.appends.pop_front();
            }
        }
    }

    /// One member-to-member call, with no deadline (`answer` adds it), and the cause of its failure.
    async fn call_peer(&self, to: SiteId, body: Vec<u8>, hop: u64) -> std::result::Result<Vec<Envelope<F>>, PeerErr> {
        let (at, name) =
            self.peer_name(to).ok_or_else(|| PeerErr::new(CallFail::Peer, format!("no member at address {}", to.0)))?;
        let site = SiteEnv::read(&self.env).map_err(|x| PeerErr::new(CallFail::SiteEnv, x))?;
        let stub = site.member(&self.env, at, &name).map_err(|x| PeerErr::new(CallFail::Stub, x))?;
        let mut init = RequestInit::new();
        init.with_method(Method::Post).with_body(Some(JsValue::from(js_sys::Uint8Array::from(body.as_slice()))));
        drop(body);
        #[cfg(feature = "test-faults")]
        {
            let h = Headers::new();
            let _ = h.set("x-graft-hop", &hop.to_string());
            init.with_headers(h);
        }
        #[cfg(not(feature = "test-faults"))]
        let _ = hop;
        let req =
            Request::new_with_init("https://member/raft", &init).map_err(|x| PeerErr::new(CallFail::Request, x))?;
        let mut res = stub.fetch_with_request(req).await.map_err(|x| PeerErr::new(CallFail::Fetch, x))?;
        let status = res.status_code();
        if status != 200 {
            let text = res.text().await.unwrap_or_default();
            return Err(PeerErr::new(CallFail::Status, format!("{status} {text}")));
        }
        let b = res.bytes().await.map_err(|x| PeerErr::new(CallFail::Body, x))?;
        codec::from_bytes::<Vec<Envelope<F>>>(&b).map_err(|x| PeerErr::new(CallFail::Decode, x))
    }

    /// Registers a waiter for `ticket`; `admitted` names the request it was admitted for, and when.
    pub fn wait_ticket(
        &self,
        ticket: Ticket,
        admitted: Option<(RequestId, u64, Option<F::C>)>,
    ) -> oneshot::Receiver<Outcome<F>> {
        let (tx, rx) = oneshot::channel();
        if let Some(core) = self.core.borrow_mut().as_mut() {
            core.waiters.entry(ticket).or_default().push(tx);
            if let Some(a) = admitted {
                core.admitted.entry(ticket).or_insert(a);
            }
        }
        rx
    }

    pub fn wait_read(&self, id: ReadId, q: Value) -> oneshot::Receiver<ReadResult> {
        let (tx, rx) = oneshot::channel();
        if let Some(core) = self.core.borrow_mut().as_mut() {
            core.reads.insert(id, (q, tx));
        }
        rx
    }

    /// Registers a waiter for the entry `Member::record` takes under `rid`.
    pub fn wait_recorded(&self, rid: RequestId) -> oneshot::Receiver<Result<Index, NotLeader>> {
        let (tx, rx) = oneshot::channel();
        if let Some(core) = self.core.borrow_mut().as_mut() {
            core.recorded.push((rid, tx));
        }
        rx
    }

    pub fn wait_block(&self, b: raft::IdBlock) -> oneshot::Receiver<Result<Index, NotLeader>> {
        let (tx, rx) = oneshot::channel();
        if let Some(core) = self.core.borrow_mut().as_mut() {
            core.blocks.push((b, tx));
        }
        rx
    }

    /// Answers a `DepsWanted`: each chain's fold, read once through its leader's `fresh` (`/read` with `dep`), found
    /// by the lookup rule, then `deps_read`. A chain that cannot be read is left out, so `decide` fails closed on it
    /// (`CredentialRefusal::NotRead`). BF-15 may hold the read back (`hold_deps_until`) so commands queue behind it.
    fn read_deps(self: &Rc<Self>, want: raft::DepsWanted) {
        let h = self.clone();
        spawn_local(async move {
            #[cfg(feature = "test-faults")]
            {
                let hold = h.core.borrow().as_ref().map_or(0, |c| c.hold_deps_until);
                if hold > now_ms() {
                    Delay::from(Duration::from_millis(hold - now_ms())).await;
                }
            }
            // Each chain at once, as "The write path" reads them: one after another cost the sum of three reads,
            // about 1.3 s of every merge (`tests/m2/results/m2-wp6.md`, "Merge latency").
            let joined = h.core.borrow().as_ref().map(|c| c.join.clone());
            let one = |chain: graftyard_types::ChainId| {
                let env = &h.env;
                let known = joined.as_ref().and_then(|j| owner_chain_record(j, chain));
                async move {
                    let t0 = now_ms();
                    let q = &json!({ "q": { "dep": true } });
                    let read = |record: graftyard_types::PlacementRecord| async move {
                        crate::lookup::call(env, chain, &record, "/read", q).await
                    };
                    // The joined record finds the group with no `Names` read first; a record the owner has since
                    // moved from may not, and then the owner's current record is read and tried.
                    let mut got = match known {
                        Some(r) => read(r).await,
                        None => Err(String::new()),
                    };
                    if got.is_err() {
                        got = match crate::release::record_for(env, chain).await {
                            Ok(r) => read(r).await,
                            Err(e) => return (None, json!({ "chain": chain, "error": format!("no record: {e}") })),
                        };
                    }
                    match got {
                        Ok((v, _)) => {
                            let bytes = v.get("value").and_then(Value::as_str).and_then(unhex);
                            match bytes.map(|b| F::dep(chain, &b)) {
                                Some(Ok(d)) => (
                                    Some((chain, d)),
                                    json!({ "chain": chain, "index": v["index"], "ms": now_ms() - t0 }),
                                ),
                                Some(Err(e)) => (None, json!({ "chain": chain, "error": e })),
                                None => (None, json!({ "chain": chain, "error": "no fold" })),
                            }
                        }
                        Err(e) => (None, json!({ "chain": chain, "error": e })),
                    }
                }
            };
            let (deps, reads): (Vec<_>, Vec<_>) = each_at_once(&want.chains, one).await.into_iter().unzip();
            let deps: Vec<_> = deps.into_iter().flatten().collect();
            h.stats.borrow_mut().dep_reads += 1;
            // The entry these reads decide is appended next, at the log's end + 1.
            #[cfg(feature = "test-faults")]
            {
                let next = h.core.borrow().as_ref().map_or(0, |c| c.m.last_index().0 + 1);
                let _ = h.state.storage().sql().exec(
                    "INSERT INTO deps_log (idx, at, reads) VALUES (?, ?, ?)",
                    vec![i(next), i(now_ms()), json!(reads).to_string().into()],
                );
            }
            let _ = &reads;
            let _ = h.input(|m, t| m.deps_read(t, want.id, deps));
        });
    }

    /// The `NeedIdBlock` lease loop: lease the next block for this chain from the cell's `Ids` group, record it here
    /// with `Member::id_block`, and once it commits, re-send every command refused for want of it under its request
    /// id. One loop at a time; commands refused meanwhile join it.
    fn lease(self: &Rc<Self>) {
        if self.leasing.replace(true) {
            return;
        }
        let h = self.clone();
        spawn_local(async move {
            let r = h.lease_once().await;
            if let Err(e) = &r {
                console_error!("id block lease failed: {e}");
            }
            h.leasing.set(false);
            let blocked = std::mem::take(&mut *h.blocked.borrow_mut());
            for (old, rid, cmd) in blocked {
                let (r2, out) = match h.step(|m, t| m.commit(t, rid, cmd.clone())) {
                    Ok(x) => x,
                    Err(_) => break,
                };
                match r2 {
                    Ok(a) => {
                        if let Some(core) = h.core.borrow_mut().as_mut() {
                            let at = now_ms();
                            core.admitted.insert(a.ticket, (rid, at, Some(cmd)));
                            let waiting = core.waiters.remove(&old).unwrap_or_default();
                            core.waiters.entry(a.ticket).or_default().extend(waiting);
                            core.reissued.insert(old, a.ticket);
                        }
                    }
                    Err(e) => {
                        if let Some(core) = h.core.borrow_mut().as_mut() {
                            let nl = match e {
                                CallError::NotLeader(nl) => nl,
                                _ => core.m.not_leader(),
                            };
                            for tx in core.waiters.remove(&old).unwrap_or_default() {
                                let _ = tx.send(Err(CommitError::NotLeader(nl.clone())));
                            }
                        }
                    }
                }
                h.dispatch(out, None);
            }
        });
    }

    async fn lease_once(self: &Rc<Self>) -> std::result::Result<(), String> {
        let chain = self.core.borrow().as_ref().map(|c| c.join.chain).ok_or("not joined")?;
        let ids = crate::release::system_record(&self.env, graftyard_types::ChainId::Ids).await?;
        let done =
            crate::lookup::commit(&self.env, graftyard_types::ChainId::Ids, &ids, json!({ "to": chain })).await?;
        let block: raft::IdBlock = serde_json::from_value(done.pointer("/ok/out").cloned().ok_or(format!("{done}"))?)
            .map_err(|e| e.to_string())?;
        self.stats.borrow_mut().leases += 1;
        let rx = self.wait_block(block);
        match self.input(|m, t| m.id_block(t, block)).map_err(|e| e.to_string())? {
            Ok(()) => {}
            // Recorded already (a re-sent lease answered from its request row).
            Err(raft::IdBlockError::Behind { .. }) => return Ok(()),
            Err(raft::IdBlockError::NotLeader(_)) => return Err("not leader".into()),
        }
        match within(Duration::from_secs(10), rx).await {
            Some(Ok(Ok(_))) => Ok(()),
            other => Err(format!("block {block:?} did not commit: {}", other.is_some())),
        }
    }

    fn has_waiters(&self) -> bool {
        self.core.borrow().as_ref().is_some_and(|c| !c.waiters.is_empty() || !c.reads.is_empty())
            || self.callers.borrow().holding()
    }

    /// Keeps `tick` running while inputs arrive or calls are held.
    pub fn kick(self: &Rc<Self>) {
        self.active_until.set(self.active_until.get().max(now_ms() + ACTIVE_FOR_MS));
        if self.ticking.replace(true) {
            return;
        }
        // The loop holds the host only between timers: after `ctx.abort()` the object's timers never fire, and a loop
        // that held it across one kept the whole reset member in the isolate's memory (F47).
        let w = Rc::downgrade(self);
        spawn_local(async move {
            loop {
                let due = now_ms() + TICK.as_millis() as u64;
                Delay::from(TICK).await;
                let Some(h) = w.upgrade() else { return };
                let lag = now_ms().saturating_sub(due);
                if lag >= 50 {
                    h.trace(Trace::Lag, 0, 0, 0, lag);
                }
                if h.dying.get() || (now_ms() > h.active_until.get() && !h.has_waiters()) || h.tick_once().is_err() {
                    h.ticking.set(false);
                    return;
                }
            }
        });
    }

    /// One `tick`, which does not count as activity, so the ticker winds down when inputs stop.
    fn tick_once(self: &Rc<Self>) -> Result<()> {
        let until = self.active_until.get();
        let r = self.step(|m, t| m.tick(t));
        self.active_until.set(until);
        let ((), out) = r?;
        self.dispatch(out, None);
        let due = self.callers.borrow_mut().due(now_ms());
        for e in due {
            self.call(e);
        }
        Ok(())
    }

    /// A leader served a client request: keep the drain-check alarm armed while it serves.
    pub fn served(self: &Rc<Self>) {
        self.served_at.set(now_ms());
        self.arm();
    }

    fn arm(self: &Rc<Self>) {
        self.arm_in(DRAIN_CHECK_MS);
    }

    #[cfg(feature = "test-faults")]
    pub fn armed_at(&self) -> u64 {
        self.alarm_at.get()
    }

    /// Arms the object's one alarm `ms` from now, unless one is set sooner.
    pub fn arm_in(self: &Rc<Self>, ms: i64) {
        let at = now_ms() + ms as u64;
        if self.alarm_at.get() != 0 && self.alarm_at.get() <= at {
            return;
        }
        self.alarm_at.set(at);
        let h = self.clone();
        spawn_local(async move {
            // Unconfirmed, so arming never holds a reply behind the output gate; a lost alarm is re-armed by the next
            // served request.
            let opts = SetAlarmOptions { allow_unconfirmed: Some(true), ..Default::default() };
            if let Err(e) = h.state.storage().set_alarm_with_options(ms, opts).await {
                let mut s = h.stats.borrow_mut();
                if s.alarms.len() < 50 {
                    s.alarms.push(format!("{} set alarm: {e}", now_ms()));
                }
            }
        });
    }

    /// The leader's alarm: one `tick`, then re-armed only while the member leads and served in the last 60 s. The
    /// family's own alarm work (a snapshot writer's batches) runs first and asks for the next alarm itself.
    pub async fn alarm(self: &Rc<Self>) {
        self.alarm_at.set(0);
        {
            let mut s = self.stats.borrow_mut();
            s.alarm_runs += 1;
            s.alarm_last = now_ms();
        }
        F::on_alarm(self).await;
        // A follower woken only for its fold work does not run Raft's timers: before the family's alarms, only a
        // member that served had one.
        let recent = now_ms().saturating_sub(self.served_at.get()) < DRAIN_IDLE_MS;
        let leading = self.core.borrow().as_ref().is_some_and(|c| c.m.role() == Role::Leader);
        if leading || recent {
            let _ = self.tick_once();
        }
        if leading && recent {
            self.arm();
        }
    }

    /// A storage write failed: reset the object. Every input from now on is refused, so nothing else leaves.
    fn fail(self: &Rc<Self>) {
        self.dying.set(true);
        abort(&self.state, "graftyard member storage write failed");
    }

    /// Whether a fold read storage refused (`Family::lost`); if so the object is resetting, and the caller must let
    /// nothing it computed from the fold leave: no reply, no batch, no snapshot part.
    pub fn fold_lost(self: &Rc<Self>) -> bool {
        let why = self.core.try_borrow().ok().and_then(|c| c.as_ref().and_then(|c| F::lost(&c.m.group_fold().fold)));
        match why {
            Some(why) => {
                self.fail_lost(&why);
                true
            }
            None => false,
        }
    }

    /// A fold read storage refused (`Family::lost`): reset this object, as a failed write does. Only this member
    /// resets; a panic would rebuild the wasm instance and reset every member in the isolate (F47's site a panic).
    fn fail_lost(self: &Rc<Self>, why: &str) {
        console_error!("member reset: fold storage lost: {why}");
        self.dying.set(true);
        abort(&self.state, "graftyard member fold storage lost");
    }

    /// The fault path: record the kill, wait until the row is durable, and reset the object after `delay`. Every
    /// input from now on is refused, so nothing else leaves.
    #[cfg(feature = "test-faults")]
    fn die(self: &Rc<Self>, fault: Option<Fault>, delay: Duration) {
        self.dying.set(true);
        let h = self.clone();
        spawn_local(async move {
            if let Some(f) = fault {
                if f.down_ms > 0 {
                    let _ = h.state.storage().sql().exec(
                        "INSERT OR REPLACE INTO me (k, v) VALUES ('down_until', ?)",
                        vec![(now_ms() + f.down_ms).to_string().into()],
                    );
                }
                let _ = h.state.storage().sql().exec(
                    "INSERT INTO fault (rid, mode, idx, term, result, at) VALUES (?, ?, ?, ?, ?, ?)",
                    vec![
                        f.rid.to_string().into(),
                        f.kill.name().into(),
                        i(f.index.0),
                        i(f.term.0),
                        f.result.to_string().into(),
                        i(now_ms()),
                    ],
                );
            }
            let _ = h.state.storage().sync().await;
            if !delay.is_zero() {
                Delay::from(delay).await;
            }
            abort(&h.state, "graftyard member fault");
        });
    }

    /// Committed entries `from..=to` from storage, at most 2,000, each with its index and the family's fields
    /// (`Family::entry_json`): what the git cache shard's `ChainLog::entries` reads.
    pub fn entries_json(&self, from: u64, to: u64) -> Result<Value> {
        let commit = self.core.borrow().as_ref().map_or(0, |c| c.m.state().commit.0);
        let to = to.min(commit);
        let sql = self.state.storage().sql();
        let r = rows(
            &sql,
            "SELECT row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx LIMIT 2000",
            vec![i(from.max(1)), i(to)],
        )?;
        let mut out = Vec::with_capacity(r.len());
        let mut bytes = 0;
        for r in r {
            // A page stops past ENTRIES_PAGE_BYTES (and always holds one entry), so a log of inline packs answers
            // well within the lookup's CALL; the shard asks again from the next index.
            let row = blob(&r[0])?;
            bytes += row.len();
            if !out.is_empty() && bytes > ENTRIES_PAGE_BYTES {
                break;
            }
            let e: Entry<F> = codec::from_bytes(row).map_err(|e| err(e.to_string()))?;
            let mut m = F::entry_json(&e);
            m.insert("index".into(), json!(e.index.0));
            m.insert("at".into(), json!(e.at.0));
            out.push(Value::Object(m));
        }
        Ok(json!(out))
    }

    /// The member's stored log, with each entry's request ids, from `from` (at most `n` entries).
    #[cfg(feature = "test-faults")]
    pub fn log_json(&self, from: u64, n: usize) -> Result<Value> {
        let core = self.core.borrow();
        let Some(core) = core.as_ref() else { return Ok(json!([])) };
        let commit = core.m.state().commit.0;
        let sql = self.state.storage().sql();
        let r = rows(
            &sql,
            "SELECT idx, digest, row FROM log WHERE idx >= ? ORDER BY idx LIMIT ?",
            vec![i(from.max(1)), i(n as u64)],
        )?;
        let mut out = Vec::with_capacity(r.len());
        for r in r {
            let e: Entry<F> = codec::from_bytes(blob(&r[2])?).map_err(|e| err(e.to_string()))?;
            let mut row = json!({
                "index": e.index.0,
                "term": e.term.0,
                "at": e.at.0,
                "digest": hex(blob(&r[1])?),
                "committed": e.index.0 <= commit,
                "kind": match &e.body {
                    EntryBody::Events(_) => "events",
                    EntryBody::Config(_) => "config",
                    EntryBody::Noop => "noop",
                    EntryBody::IdBlock(_) => "idBlock",
                },
                "requests": e.requests.iter().map(|(r, _)| r.to_string()).collect::<Vec<_>>(),
            });
            // The family's fields at the top level, as `tests/m1/wp6/bf17.sh` reads `bytes` and `ref_updates`.
            for (k, v) in F::entry_json(&e).into_iter().chain(F::log_fields(&e)) {
                row[k] = v;
            }
            out.push(row);
        }
        Ok(json!(out))
    }

    /// BF-12's audit over the committed log on storage: request ids at more than one index, `dup` rows written by
    /// apply, and every register whose key starts with `prefix`, in memory and on disk.
    #[cfg(feature = "test-faults")]
    pub fn audit_json(&self, prefix: &str) -> Result<Value> {
        let core = self.core.borrow();
        let core = core.as_ref().ok_or_else(|| err("member not joined"))?;
        let commit = core.m.state().commit.0;
        let sql = self.state.storage().sql();
        let mut at: BTreeMap<RequestId, Vec<u64>> = BTreeMap::new();
        let mut from = 1;
        while from <= commit {
            let page = rows(
                &sql,
                "SELECT row FROM log WHERE idx >= ? AND idx <= ? ORDER BY idx LIMIT 2000",
                vec![i(from), i(commit)],
            )?;
            if page.is_empty() {
                break;
            }
            for r in &page {
                let e: Entry<F> = codec::from_bytes(blob(&r[0])?).map_err(|e| err(e.to_string()))?;
                from = e.index.0 + 1;
                for (r, _) in &e.requests {
                    at.entry(*r).or_default().push(e.index.0);
                }
            }
        }
        let twice: Vec<Value> =
            at.iter().filter(|(_, v)| v.len() > 1).map(|(r, v)| json!({ "rid": r.to_string(), "at": v })).collect();
        let dup = rows(&sql, "SELECT rid, idx FROM dup", vec![])?
            .iter()
            .map(|r| Ok(json!({ "rid": text(&r[0])?, "idx": int(&r[1])? })))
            .collect::<Result<Vec<_>>>()?;
        let request_rows = int(&rows(&sql, "SELECT count(*) FROM request", vec![])?[0][0])?;
        let fold = F::audit(&sql, &core.m.group_fold().fold, prefix)?;
        Ok(json!({
            "last": core.m.last_index().0,
            "base": core.m.log_base().index.0,
            "commit": commit,
            "applied": core.applied.0,
            "requestIds": at.len(),
            "requestRows": request_rows,
            "logTwice": twice,
            "dupRows": dup,
            "values": fold.get("values"),
            "diskValues": fold.get("diskValues"),
        }))
    }
}

/// How long this install keeps `request` rows: `REQUEST_RETENTION`, or with `test-faults` the
/// `GRAFTYARD_REQUEST_RETENTION_S` variable, which every site of an install sets to the same value.
pub fn request_retention(env: &Env) -> Duration {
    #[cfg(feature = "test-faults")]
    if let Some(s) = env.var("GRAFTYARD_REQUEST_RETENTION_S").ok().and_then(|v| v.to_string().parse().ok()) {
        return Duration::from_secs(s);
    }
    let _ = env;
    raft::REQUEST_RETENTION
}

/// `ctx.abort()`: resets the object. workers-rs 0.8.7 does not bind it. The call is deferred to a timer that runs
/// `abort` bound to the state, so it throws on a stack with no wasm frame: called from inside wasm, the uncatchable
/// error unwound through the module, the shim marked the instance terminated, and the object hung on every later
/// request instead of resetting (measured on the first BF-12 kill).
pub fn abort(state: &State, why: &str) {
    let raw: &JsValue = state.as_raw().as_ref();
    let global = js_sys::global();
    let abort =
        js_sys::Reflect::get(raw, &JsValue::from_str("abort")).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    let timer = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
        .ok()
        .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
    if let (Some(abort), Some(timer)) = (abort, timer) {
        let bound = abort.bind1(raw, &JsValue::from_str(why));
        let _ = timer.call2(&global, &bound, &JsValue::from(0));
    }
}

// ---- JSON forms of the crate's replies ----

pub fn nl_json(nl: &NotLeader) -> Value {
    json!({
        "term": nl.term.0,
        "leader": nl.leader.map(|s| s.0),
        "config": nl.config.as_ref().map(|(i, c)| json!({ "index": i.0, "config": c })),
    })
}

pub fn outcome_json<F: Family>(fold: &F::Fold, o: &Outcome<F>) -> Value {
    match o {
        Ok(a) => json!({ "ok": {
            "index": a.index.0,
            "out": F::out_json(fold, a),
            "quorum": a.quorum().sites().iter().map(|s| s.0).collect::<Vec<_>>(),
        } }),
        Err(CommitError::Refused { index, err: e }) => {
            let mut r = F::err_json(e);
            r["index"] = json!(index.0);
            json!({ "refused": r })
        }
        Err(CommitError::NotLeader(nl)) => json!({ "notLeader": nl_json(nl) }),
        Err(CommitError::Full { retry_after_s }) => json!({ "full": { "retry_after_s": retry_after_s } }),
    }
}

/// An `Acked`'s evidence, on this member's clock (`tests/m1/results/m1-wp2.md`, "Member-side ack evidence"): the
/// write's entry, the commit its `QuorumProof` is for, when the leader built that entry and committed it, when this
/// reply was built (`sentAt`; the reply leaves at or after it), when this host's own storage confirm first covered the
/// commit (`selfSyncedAt`, the host's record, not the crate's), and each counted site's durable index with when the
/// leader learned it. `tests/m1/lin/ackcheck.ts` checks every acknowledged write's.
impl<F: Family> Host<F> {
    pub fn ack_json(&self, a: &raft::Acked<FOut<F>>) -> Value {
        let p = a.quorum();
        let synced = self.synced.borrow().iter().filter(|r| r.0 >= p.index().0).map(|r| r.1).min();
        json!({
            "site": p.leader().0,
            "index": a.index.0,
            "commitIndex": p.index().0,
            "entryAt": p.entry_at().0,
            "commitAt": p.at().0,
            "sentAt": now_ms(),
            "selfSyncedAt": synced,
            "acks": p.acks().iter().map(|d| json!({ "site": d.site.0, "durable": d.durable.0, "at": d.at.0 }))
                .collect::<Vec<_>>(),
        })
    }

    /// `outcome_json` with the `Acked`'s evidence, for a reply this member sends now.
    pub fn reply_json(&self, o: &Outcome<F>) -> Value {
        let core = self.core.borrow();
        let Some(c) = core.as_ref() else { return json!({ "notJoined": true }) };
        let mut v = outcome_json::<F>(&c.m.group_fold().fold, o);
        drop(core);
        if let Ok(a) = o {
            v["ok"]["ack"] = self.ack_json(a);
        }
        v
    }
}

/// A refusal the lease loop answers by re-sending, not the caller.
pub fn needs_block<F: Family>(o: &Outcome<F>) -> bool {
    matches!(o, Err(CommitError::Refused { err: e, .. }) if F::needs_block(e))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    s.len()
        .is_multiple_of(2)
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())
        .flatten()
}

pub fn call_error_json(e: &CallError) -> Value {
    match e {
        CallError::NotLeader(nl) => json!({ "notLeader": nl_json(nl) }),
        CallError::Unsupported(u) => {
            json!({ "unsupported": { "family": u.format.family, "version": u.format.version } })
        }
        CallError::TooLarge { len, max } => json!({ "tooLarge": { "len": len, "max": max } }),
    }
}

pub fn ticket_json(t: &Ticket) -> Value {
    json!({ "term": t.term.0, "boot": t.boot.to_string(), "seq": t.seq })
}

pub fn ticket_from(v: &Value) -> Result<Ticket> {
    let term = v.get("term").and_then(Value::as_u64).ok_or_else(|| err("ticket.term"))?;
    let boot = v.get("boot").and_then(Value::as_str).and_then(|s| s.parse().ok()).ok_or_else(|| err("ticket.boot"))?;
    let seq = v.get("seq").and_then(Value::as_u64).ok_or_else(|| err("ticket.seq"))?;
    Ok(Ticket { term: raft::Term(term), boot, seq })
}

#[cfg(feature = "test-faults")]
pub fn digest_hex(d: &raft::Digest) -> String {
    d.hex()
}

/// The record a repository member already holds for one of its dependency chains: its owner's `Authz` is under the
/// owner's record, the one the member joined with, so the leader reads it with no `Placement` read on `Names` first
/// (one cross-region round trip of every merge on a weur or apac leader, `tests/m2/results/m2-wp6.md`).
fn owner_chain_record(join: &Join, chain: graftyard_types::ChainId) -> Option<graftyard_types::PlacementRecord> {
    match (join.chain, chain) {
        (graftyard_types::ChainId::Repo(_), graftyard_types::ChainId::Authz(o)) if join.owner == Some(o) => {
            Some(join.record.clone())
        }
        _ => None,
    }
}

/// `read` of every chain, all started at once and awaited together, in `chains` order (`design/DESIGN.md`, "The write
/// path": "the leader reads each distinct dependency ... once through `fresh`, in parallel").
async fn each_at_once<T, Fut: std::future::Future<Output = T>>(
    chains: &[graftyard_types::ChainId],
    read: impl FnMut(graftyard_types::ChainId) -> Fut,
) -> Vec<T> {
    futures_util::future::join_all(chains.iter().copied().map(read)).await
}

/// A request id in its text form, the ULID's 26 characters.
pub fn rid_from(s: &str) -> Result<RequestId> {
    s.parse().map_err(|_| err(format!("bad request id {s:?}")))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use futures_channel::oneshot;

    use super::{Answer, answer, each_at_once};

    /// A future that ends when its sender fires.
    fn gate() -> (oneshot::Sender<()>, impl std::future::Future<Output = ()>) {
        let (tx, rx) = oneshot::channel::<()>();
        (tx, async move {
            let _ = rx.await;
        })
    }

    /// An AppendEntries answer that arrives after CALL_DEADLINE: the miss is reported when the deadline passes, and the
    /// answer is still returned for the core to step. Dropping it is what kept `repo:196608`'s site-0 member at 235
    /// (`tests/m1/results/m1-f43.md`).
    #[test]
    fn an_answer_after_the_deadline_is_kept_and_the_miss_reported_first() {
        let (answer_tx, answer_rx) = oneshot::channel::<u64>();
        let (deadline_tx, deadline) = gate();
        let (_limit_tx, limit) = gate();
        let missed = Cell::new(false);
        let call = async move { answer_rx.await.unwrap() };
        let mut f = pin!(answer(call, deadline, limit, || missed.set(true)));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Pending);
        deadline_tx.send(()).unwrap();
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Pending, "still waiting for the answer");
        assert!(missed.get(), "the miss is reported at the deadline, before the answer");
        answer_tx.send(235).unwrap();
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Ready(Answer::Late(235)));
    }

    /// Every dependency read is in flight before any finishes: each read here ends only once all three started, so
    /// reads made one after another never end (each waited for the one before, about 1.3 s of every merge,
    /// `tests/m2/results/m2-wp6.md`, "Merge latency").
    #[test]
    fn dependency_reads_run_at_once() {
        use graftyard_types::{AccountId, ChainId, DbId};
        let started = Cell::new(0);
        let chains: Vec<ChainId> = (1..=3).map(|n| ChainId::Authz(AccountId(DbId(n)))).collect();
        let mut f = pin!(each_at_once(&chains, |c| {
            let started = &started;
            let mut first = true;
            std::future::poll_fn(move |_| {
                if std::mem::take(&mut first) {
                    started.set(started.get() + 1);
                }
                if started.get() == 3 { Poll::Ready(c) } else { Poll::Pending }
            })
        }));
        let mut cx = Context::from_waker(Waker::noop());
        let mut done = None;
        for _ in 0..3 {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                done = Some(v);
                break;
            }
        }
        let want: Vec<ChainId> = (1..=3).map(|n| ChainId::Authz(AccountId(DbId(n)))).collect();
        assert_eq!(done, Some(want));
    }

    /// A repository leader reads its owner's `Authz` under the record it joined with, so a merge's dependency reads
    /// start with no `Placement` read on `Names`; any other chain, or another owner's `Authz`, is looked up.
    #[test]
    fn the_owners_authz_is_read_under_the_joined_record() {
        use graftyard_types::{AccountId, ChainId, DbId, MemberKind, PlacementName, PlacementRecord, RepoId, SiteId};
        let record = PlacementRecord {
            name: PlacementName::Tri,
            sites: [0, 1, 2].into_iter().map(SiteId).collect(),
            home: SiteId(0),
            generation: 4,
            moving_from: None,
            at: graftyard_types::Timestamp(1),
        };
        let (owner, other) = (AccountId(DbId(7)), AccountId(DbId(8)));
        let chain = ChainId::Repo(RepoId(DbId(9)));
        let join = super::Join {
            chain,
            me: graftyard_types::Member::new(chain, SiteId(1), MemberKind::Full, 0, 4),
            record: record.clone(),
            peers: Vec::new(),
            owner: Some(owner),
            genesis: None,
            #[cfg(feature = "test-faults")]
            no_claim: false,
        };
        assert_eq!(super::owner_chain_record(&join, ChainId::Authz(owner)), Some(record));
        assert_eq!(super::owner_chain_record(&join, ChainId::Authz(other)), None);
        assert_eq!(super::owner_chain_record(&join, ChainId::Tokens(graftyard_types::TokenShard(3))), None);
    }

    #[test]
    fn an_answer_in_time_reports_no_miss_and_one_past_the_limit_is_missed() {
        let missed = Cell::new(false);
        let (_deadline_tx, deadline) = gate();
        let (_limit_tx, limit) = gate();
        let mut f = pin!(answer(async { 7 }, deadline, limit, || missed.set(true)));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Ready(Answer::InTime(7)));
        assert!(!missed.get());

        let (_answer_tx, answer_rx) = oneshot::channel::<u64>();
        let (deadline_tx, deadline) = gate();
        let (limit_tx, limit) = gate();
        let mut f = pin!(answer(async move { answer_rx.await.unwrap() }, deadline, limit, || missed.set(true)));
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Pending);
        deadline_tx.send(()).unwrap();
        limit_tx.send(()).unwrap();
        assert_eq!(f.as_mut().poll(&mut cx), Poll::Ready(Answer::Missed));
        assert!(missed.get());
    }
}
