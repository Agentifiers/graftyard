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
//!   Shape, the drain check). `tick` starts no campaign; campaigns start from `/campaign` and `/suspect`, and from a
//!   voter's own alarm when it knows no leader while its group has leader-only work due (`keep_led`).
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
use worker::wasm_bindgen::{JsCast, JsValue};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::{Err as FErr, Family, Out as FOut, Stmt};
use crate::member::Join;
use crate::{SiteEnv, now_ms};

type Entry<F> = raft::LogEntry<<F as Family>::Ev>;
type Envelope<F> = raft::Envelope<<F as Family>::Ev>;

pub type Outcome<F> = Result<Acked<FOut<F>>, CommitError<FErr<F>>>;
pub type ReadResult = Result<(ReadBody, Index), NotLeader>;

/// A granted read's answer. `Json` is its `value` already serialized: a grant holds only until the member's next
/// input (`raft::ReadGrant`), so the answer is built in the input that granted it, as bytes, and the reads of one
/// query granted in the same input share one copy (`shared_answer`). The `Value` tree is dropped at once, so one
/// quorum round that grants hundreds of reads no longer holds hundreds of trees until each is sent: 600 refs reads
/// granted together took an idle leader from 56 to 94 MB, and on B0-29 600 reads ended in one millisecond before
/// the apac leader's `exceededMemory` (`tests/am1/results/lm1-memory.md`, "Rows"). `Shed`: a whole-tier or narrow
/// read whose rebuild is not due yet (`family::dump_wait`, `family::narrow_wait`), with the ms until it is and the
/// read's grant, which `Host::answer_after` answers later without another read-index round.
#[derive(Clone, Debug, PartialEq)]
pub enum ReadBody {
    Json(Rc<[u8]>),
    Shed(u64, Option<raft::ReadGrant>),
}

/// The answer for query `q` granted at `floor` from `built`, the answers already made in this input, else from
/// `build`, kept for the input's other reads of the same query and floor. `built` must not outlive the input: its
/// answers are at that input's grant. The floor is part of the key because a cached narrow answer may be older than
/// the input's state, and is good only for reads whose floor it covers (`family::narrow_bytes`).
pub fn shared_answer(
    built: &mut HashMap<String, ReadBody>,
    q: &Value,
    floor: u64,
    build: impl FnOnce() -> ReadBody,
) -> (ReadBody, bool) {
    let key = format!("{floor} {q}");
    if let Some(b) = built.get(&key) {
        return (b.clone(), true);
    }
    let b = build();
    built.insert(key, b.clone());
    (b, false)
}

/// A granted read's answer from the group's fold at `now`, for a member that spaces dump rebuilds by `every`
/// (`family::dump_every`): `Shed` when a stale dump's rebuild is not due (checked here, where the dump would be
/// built, so one granted batch rebuilds at most once per `every`); else the family's shared bytes (`read_bytes`, one
/// copy for every read at this state, from any input); else `read_json` serialized.
pub fn fold_answer<F: Family>(fold: &F::Fold, q: &Value, floor: u64, now: u64, every: u64) -> ReadBody {
    if let Some(ms) = F::heavy_wait(fold, q, floor, now, every) {
        return ReadBody::Shed(ms, None);
    }
    if let Some(b) = F::read_bytes(fold, q, floor) {
        return ReadBody::Json(b);
    }
    ReadBody::Json(serde_json::to_vec(&F::read_json(fold, q)).unwrap_or_else(|_| b"null".to_vec()).into())
}

const TICK: Duration = Duration::from_millis(100);
const ACTIVE_FOR_MS: u64 = 3_000;
const DRAIN_CHECK_MS: i64 = 2_000;
pub const DRAIN_IDLE_MS: u64 = 60_000;
/// Restore compares the fold loaded from its tables with the log replayed through `applied`, while at most this many
/// entries are applied. Longer logs skip it: replaying them would hold every request row the log ever carried.
const RESTORE_CHECK_MAX: u64 = 20_000;
/// Latency samples kept for `/stats`.
pub const SAMPLES_MAX: usize = 200_000;

/// A command admitted at `admitted` and appended at `appended`, with the leader's last apply before the append at
/// `last_apply`: (`slot`, `post`) ms, summing to its queue time. Admitted after that apply, it waited for no entry
/// ahead of it, so its whole queue is `post`; admitted before it, `slot` runs to the apply and `post` from it.
fn queue_split(admitted: u64, last_apply: u64, appended: u64) -> (u32, u32) {
    let freed = last_apply.clamp(admitted, appended);
    ((freed - admitted) as u32, (appended - freed) as u32)
}

/// An input's two clocks: the member clock it stepped at (`Host::now`, floored by peer stamps) and this isolate's own
/// (`now_ms()`), which a busy isolate reads up to seconds behind the member clock (`crate::clock`).
#[derive(Clone, Copy, Debug)]
struct Clocks {
    member: u64,
    raw: u64,
}
/// One leader confirm split, LM-1 round 12's question of why B0-33's confirm p99 reached 1.7 s: `gate`, the input's
/// step to the start of the transaction that carried its writes (the deferred flush's macrotask, so the isolate's turn
/// queue); `sync`, that start to `sync()`'s continuation (storage, plus the continuation's own turn); `turn`, a
/// zero-delay timer set at the same start to its firing, capped at `sync` (the turn queue then, so `sync - turn` is
/// about storage alone); and the transaction's statements and argument bytes. Workers' clock advances only across
/// I/O, so the synchronous `transactionSync` reads as 0 ms here and shows up as other inputs' turn wait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfirmSplit {
    pub gate: u32,
    pub sync: u32,
    pub turn: u32,
    pub stmts: u32,
    pub bytes: u32,
}

/// The kinds of statement in a transaction of 100 KB or more (`CONFIRM_BIG`), each its count and argument bytes, keyed
/// by the statement's verb and table (`stmt_kind`): which writes a large confirm carried (LM-1 round 14's 416 to 834
/// KB confirms of up to 1,560 statements). The newest `BIG_KEPT` are kept.
pub type BigTx = (u32, u32, Vec<(String, u32, u32)>);
const CONFIRM_BIG: u32 = 100_000;
const BIG_KEPT: usize = 20;

/// A statement's verb and table: `INSERT log`, `DELETE request`, `INSERT "core_t3_pr"`.
fn stmt_kind(q: &str) -> String {
    let w: Vec<&str> = q.split_whitespace().collect();
    let verb = w.first().copied().unwrap_or("?");
    let table = w
        .iter()
        .position(|x| x.eq_ignore_ascii_case("INTO") || x.eq_ignore_ascii_case("FROM"))
        .and_then(|i| w.get(i + 1))
        .map(|t| t.split('(').next().unwrap_or(t).trim_matches('"'))
        .unwrap_or("?");
    format!("{verb} {table}")
}

/// Each statement kind's count and argument bytes in `stmts`, largest bytes first.
fn stmt_mix(stmts: &[Stmt]) -> Vec<(String, u32, u32)> {
    let mut by: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for st in stmts {
        let e = by.entry(stmt_kind(st.0)).or_default();
        e.0 += 1;
        e.1 = e.1.saturating_add(stmt_bytes(std::slice::from_ref(st)).1);
    }
    let mut v: Vec<(String, u32, u32)> = by.into_iter().map(|(k, (n, b))| (k, n, b)).collect();
    v.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Transaction sizes `confirm_split_summary` groups by: under 100 KB, under 1 MB, and the rest [guess for the bounds:
/// ENTRY_SOFT is 100 KB, and a human pack is about 1 MiB].
const CONFIRM_BYTES: [(&str, u32); 3] = [("lt100k", 100_000), ("lt1m", 1_000_000), ("ge1m", u32::MAX)];

/// `/stats?summary=1`'s `confirmSplit`: for each transaction size group, the count and the p50 and p99 of `gate`,
/// `sync`, `turn`, `sync - turn` (`storage`), the statements, and the bytes.
pub fn confirm_split_summary(xs: &[ConfirmSplit]) -> Value {
    let pct = |mut v: Vec<u32>| {
        v.sort_unstable();
        let at = |p: f64| v.get(((p * v.len() as f64) as usize).min(v.len().saturating_sub(1))).copied();
        json!({ "p50": at(0.5), "p99": at(0.99) })
    };
    let mut out = serde_json::Map::new();
    let mut lo = 0;
    for (name, hi) in CONFIRM_BYTES {
        let g: Vec<&ConfirmSplit> = xs.iter().filter(|c| c.bytes >= lo && c.bytes < hi).collect();
        lo = hi;
        if g.is_empty() {
            continue;
        }
        let f = |k: fn(&ConfirmSplit) -> u32| pct(g.iter().map(|c| k(c)).collect());
        out.insert(
            name.to_string(),
            json!({
                "n": g.len(),
                "gate": f(|c| c.gate),
                "sync": f(|c| c.sync),
                "turn": f(|c| c.turn),
                "storage": f(|c| c.sync.saturating_sub(c.turn)),
                "stmts": f(|c| c.stmts),
                "bytes": f(|c| c.bytes),
            }),
        );
    }
    Value::Object(out)
}

/// `/stats?summary=1`'s `admittedBytes`: per class, the count, p50, p99 and max of the priced bytes, their sum, and the
/// sum of those at or over 10 KB, so a reader sees which commands move `Admission::cmd_bytes`' mean.
pub fn admitted_bytes_summary(by: &BTreeMap<&'static str, Vec<u32>>) -> Value {
    let mut out = serde_json::Map::new();
    for (k, xs) in by {
        let mut s = xs.clone();
        s.sort_unstable();
        let at = |p: f64| s.get(((p * s.len() as f64) as usize).min(s.len().saturating_sub(1))).copied();
        let sum: u64 = s.iter().map(|&x| u64::from(x)).sum();
        let big: u64 = s.iter().filter(|&&x| x >= 10_000).map(|&x| u64::from(x)).sum();
        out.insert(
            k.to_string(),
            json!({ "n": s.len(), "p50": at(0.5), "p99": at(0.99), "max": s.last(), "sum": sum, "sumOver10k": big }),
        );
    }
    Value::Object(out)
}

/// A transaction's statements and argument bytes, for `ConfirmSplit`.
fn stmt_bytes(stmts: &[Stmt]) -> (u32, u32) {
    let b: usize = stmts
        .iter()
        .flat_map(|(_, a)| a)
        .map(|v| match v {
            SqlStorageValue::Blob(b) => b.len(),
            SqlStorageValue::String(s) => s.len(),
            _ => 8,
        })
        .sum();
    (stmts.len() as u32, b.min(u32::MAX as usize) as u32)
}

/// Storage confirms and truncations kept for `ack_json`'s `selfSyncedAt`: an `Acked` is answered within seconds of
/// its commit, and a confirm row a minute old covers every index at or below it.
const SYNCED_KEEP: usize = 256;
/// Stored bytes per `/entries` page. 239 entries of `wp6/main` were 4.6 MB of JSON and took 7.8 s to answer through
/// the test edge, past the lookup's 3.5 s CALL, so the git cache shard's rehydrate never finished
/// (`tests/m1/results/m1-wp8.md`, t-256). A page this size is under 1 MB of JSON [guess for the bound].
pub const ENTRIES_PAGE_BYTES: usize = 600_000;
/// The most stored bytes a local `/entries` page may ask for (`bytes=`): a git cache shard reading its own site's
/// member, inside the site, under the lookup's 3.5 s CALL. A hot group's entry is about one 500 KB inline pack, so a
/// 600 KB page held one entry, and a rehydrate's tail ran at one member read per pack, about 120 ms each
/// (`tests/am1/results/hot-shards-2.md`). The shard holds two pages at once, so this bounds its memory too.
pub const ENTRIES_LOCAL_PAGE_BYTES_MAX: usize = 4_000_000;
/// The most a local page may hold while the member is serving other reads (`ENTRIES_BUSY_READS` or more in flight):
/// a 4 MB page built beside them took site b's member near its 128 MB, and it trapped "memory access out of bounds"
/// 3 s after a home-shard kill, F47's shape (`tests/am1/results/hot-shards-3.md`, "Site b's trap") [guess for 1 MB].
pub const ENTRIES_BUSY_PAGE_BYTES: usize = 1_000_000;
/// Reads in flight (`Host::reads_in_flight`) at which a local page drops to `ENTRIES_BUSY_PAGE_BYTES` [guess]: an idle
/// member, such as one under a rehydrate with no pushes, serves the 4 MB page.
pub const ENTRIES_BUSY_READS: u32 = 1;

/// The page an `/entries` read gets: a local read (the site's own git cache shard) the `bytes` it asks for, within
/// `ENTRIES_PAGE_BYTES..=ENTRIES_LOCAL_PAGE_BYTES_MAX`, or at most `ENTRIES_BUSY_PAGE_BYTES` while `reads` other reads
/// are in flight; a remote one, which crosses regions under the lookup's CALL, always `ENTRIES_PAGE_BYTES`.
pub fn entries_page_bytes(local: bool, asked: Option<u64>, reads: u32) -> usize {
    let max = if reads >= ENTRIES_BUSY_READS { ENTRIES_BUSY_PAGE_BYTES } else { ENTRIES_LOCAL_PAGE_BYTES_MAX };
    match asked {
        Some(b) if local => b.clamp(ENTRIES_PAGE_BYTES as u64, max as u64) as usize,
        _ => ENTRIES_PAGE_BYTES,
    }
}

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS raft_state (id INTEGER PRIMARY KEY CHECK (id = 1), row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, digest BLOB NOT NULL, row BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS reg (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
    crate::release::RELEASE_SCHEMA,
    crate::release::RELEASE_AT_SCHEMA,
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

/// The log line of a `STEP_DOWN` trip's window: `{"stepDownWindow": {from, to, slow, of, confirms}}`, where `slow` counts
/// the confirms over `confirm_p99_ms`, `confirms` lists each as [end, ms] on the member clock, and `kept` the last 30 s.
pub(crate) fn step_down_line(w: &graftyard_raft::StepDownWindow) -> String {
    let slow = w.confirms.iter().filter(|c| c.1 > graftyard_raft::STEP_DOWN.confirm_p99_ms).count();
    let pairs = |v: &[(graftyard_types::Timestamp, u32)]| -> Vec<[u64; 2]> {
        v.iter().map(|(t, ms)| [t.0, u64::from(*ms)]).collect()
    };
    json!({ "stepDownWindow": { "from": w.from.0, "to": w.to.0, "slow": slow, "of": w.confirms.len(),
        "confirms": pairs(&w.confirms), "kept": pairs(&w.kept) } })
    .to_string()
}

/// Keys of `Stats::shed_by`, in count order.
pub const SHED_GATES: [&str; 6] = ["readsMax", "answerBytes", "heavyReads", "rebuild", "late", "forgot"];

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
    /// Leader, per input that appended an `Events` entry: its `at` (the step's clock, which judges every lease the entry
    /// decides, `ExtendLeases` and expiry) to `now_ms()` once the step returned, and to one macrotask later. A busy
    /// isolate whose clock reads stale shows a gap here (L0-12 m3fr2: the leader's `/status` read 350 to 800 ms below
    /// both followers). Test installs only.
    #[cfg(feature = "test-faults")]
    pub lease_clock: Vec<[u32; 2]>,
    /// Test installs: this member's election trace, oldest first, each `[at, kind, term, peer, index, extra]`
    /// (`Elect`), at most ELECT_MAX. Kept apart from `trace`, which a leader's appends fill in seconds.
    #[cfg(feature = "test-faults")]
    pub elections: VecDeque<[u64; 6]>,
    /// Test installs: the most counted bytes this member held at once since `/stats?high=1` last read them
    /// (`Host::mark_mem`), and the total its last `mem high` log line named.
    #[cfg(feature = "test-faults")]
    pub mem_high: MemHigh,
    #[cfg(feature = "test-faults")]
    pub mem_logged: usize,
    /// Leader: storage confirm of an input that appended an entry (its writes to `storage.sync()`), in ms. The output
    /// gate holds the entry's AppendEntries for this long.
    pub confirm_ms: Vec<u32>,
    /// Leader: each timed confirm split (`ConfirmSplit`), for `confirm_split_summary`.
    pub confirm_split: Vec<ConfirmSplit>,
    /// Every member: the statement mix of its newest transactions of `CONFIRM_BIG` or more (`BigTx`).
    pub big_tx: Vec<BigTx>,
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
    /// `/read`s this member received since `inbound_since`, by `kind:plain|index:f|l` (`crate::member`'s `read_kind`;
    /// `f` when `Family::follower_serves` would let a follower answer it): which leader reads could go to learners.
    pub reads_by: BTreeMap<String, u32>,
    pub failed_calls: u32,
    pub dep_reads: u32,
    /// Leader: each `DepsWanted` read, `DepsWanted` to `deps_read`, in ms, under `all`, and each chain's own read under
    /// its kind (`authz`, `identity`, `tokens`, ...); `depFails` counts the chain reads that gave no fold.
    pub deps_ms: BTreeMap<String, Vec<u32>>,
    pub dep_fails: BTreeMap<String, u32>,
    /// Leader: dependency reads that sent a second request after DEP_HEDGE, and those the second answered
    /// (`graftyard_lookup::call_hedged`).
    pub dep_hedges: [u32; 2],
    /// Leader: a command's admission to the append of the entry that carries it, in ms: the wait for the entry in
    /// flight, the cut, and the entry's dep reads. `commit_ms` less this is the entry's confirm and quorum round.
    pub queue_ms: Vec<u32>,
    /// Leader: `queue_ms` split at the apply of the entry before the command's own (`queue_split`): `slot`, the wait
    /// for the entries ahead of it, and `post`, that apply to its own entry's append (the cut, the dep reads that
    /// outlast the entry in flight, and the decide). LM-1 round 10's plan for B0-29's commit line.
    pub queue_slot_ms: Vec<u32>,
    pub queue_post_ms: Vec<u32>,
    /// Leader: at each `Events` append of its term, the frozen batches still held behind it (`held_at_append`), and
    /// the time since its previous such append (`cycle_ms`): `read-ahead`'s flip is a cycle that stays read-bound with
    /// two batches held (B0-31).
    pub held_at_append: Vec<u32>,
    pub cycle_ms: Vec<u32>,
    last_append_at: Option<(u64, u64)>,
    /// Leader: when it last applied an entry, by this isolate's own clock (`Host::note`).
    last_apply_at: u64,
    /// Reads shed past `crate::member::READS_MAX` or `HEAVY_READS`, or before a dump rebuild was due.
    pub shed_reads: u32,
    /// Reads shed by gate or later overload cause, indexed by `SHED_GATES`.
    pub shed_by: [u32; 6],
    /// Every stamp's lag (`crate::clock`), stamp minus the member clock at its arrival, by bucket: 0, then below 2^i
    /// ms for i from 1, the last bucket open; and the largest seen.
    pub clock_lag: [u32; 16],
    pub clock_lag_max: u64,
    /// Leader: reads that started while the clock was suspect, so waited for a round instead of the lease.
    pub lease_fallbacks: u32,
    /// Local `/entries` pages capped at `ENTRIES_BUSY_PAGE_BYTES` because other reads were in flight.
    pub busy_pages: u32,
    /// Reads answered from another read of the same query granted in the same input (`shared_answer`).
    pub shared_reads: u32,
    /// A learner's whole-tier reads, and any read-index member's narrow reads, that waited for a rebuild instead of
    /// being shed, counted once per wait.
    pub waited_reads: u32,
    /// Read-index reads this member answered, each `[grant, waited, total]` in ms: arrival to the leader's read index
    /// applied and granted, the time spent waiting for a rebuild, and arrival to the answer (narrow-reads-3).
    pub read_stages: Vec<[u32; 3]>,
    /// The ms each narrow API answer took to build (`family::narrow_bytes`).
    pub narrow_build_ms: Vec<u32>,
    /// Read-index reads this member served as a follower, refused to the leader (`Family::follower_serves`), and
    /// answered `overloaded` after FOLLOWER_WAIT (`crate::member`).
    pub follower_reads: u32,
    pub follower_refused: u32,
    /// Whole-tier and `ls-refs` read-index reads this member refused while it led (`tier-off-leader`).
    pub leader_refused: u32,
    pub follower_late: u32,
    /// Reads served by `stale()` under a `StaleMode` flag (`crate::member`, `read_stale`).
    pub stale_reads: u32,
    /// Stored entries sent to a follower behind the in-memory window (`Out::catchup`).
    pub catchups: u32,
    pub leases: u32,
    /// `Committed` marker puts that failed after an ok push (the sweeper repairs them).
    pub marker_failures: u32,
    /// The stored log's first row after the last truncation (`truncate`).
    pub truncated_to: u64,
    /// Leader: archive alarm runs that promoted or archived a range (`crate::archive`).
    pub archive_steps: u32,
    /// Campaigns this member started with no request, for leader-only work due (`Host::keep_led`).
    pub keep_led: u32,
    /// Storage writes SQLite refused as full (`Host::full`), in this isolate.
    pub sqlite_full: u32,
    /// Commands this member received, by format (`recv:<family>.<version>`), and `recv:unsupported` for bodies whose
    /// command it could not decode.
    pub received: BTreeMap<String, u32>,
    /// Leader: each admitted command's priced bytes (`stored_len`, less an Agent inline pack), by the class
    /// it queued under (`admitted_bytes_summary`).
    pub admitted_bytes: BTreeMap<&'static str, Vec<u32>>,
    pub alarms: Vec<String>,
    /// Object alarms that fired, and when the last one did.
    pub alarm_runs: u32,
    pub alarm_last: u64,
    pub alarm_arms: u32,
    /// `PrSync` batches this member's leader alarm sent to its site's queue, the re-send path.
    pub prsync_sent: u32,
    /// `PrSync` first attempts this member's leader alarm ran in band.
    pub prsync_first: u32,
    /// The cursor at which this member's leader alarm parked `PrSync` (`PARK_AFTER`), 0 when not parked.
    pub prsync_parked: u64,
    /// Leader: the network's test-merge budget counts (`graftyard_git::prsync::BudgetStats`) as the shard last sent
    /// them to one of this member's first attempts. Cumulative on the shard since its `since`, so `/stats?reset`
    /// keeps it.
    pub test_merges: Option<serde_json::Value>,
    /// Leader: head-push test merges its first attempts asked the shard for, and how many came back shed.
    pub test_merge_asks: u32,
    pub test_merge_shed: u32,
    appended: Vec<(u64, u64)>,
    /// Every member-to-member call this member sent, by the peer's site id.
    pub peers: BTreeMap<u8, PeerCalls>,
    /// Every member-to-member call this member sent, by message kind: send to answer, in ms (B0-18's trace).
    pub calls: BTreeMap<&'static str, CallTimes>,
    /// Test installs: B0-18's read-index trace, oldest first, each `[at, kind, peer, batch, index, ms]` (`Trace`).
    pub trace: VecDeque<[u64; 6]>,
    /// Test installs: member-to-member calls by modelled hop (`Host::hop`): `[ok, failed for depth, failed otherwise]`.
    pub hops: BTreeMap<u64, [u32; 3]>,
    /// Follower: the last RAFT_IN `/raft` calls of 1 KB or more, each `[at, bytes, body ms, work ms]`: the request
    /// body's arrival, then decode, step, and the output gate (B0-02's per-KB append cost, `tests/m0/results`).
    pub raft_in: VecDeque<[u64; 4]>,
    /// Leader: groups this member placed with no try within `NEAR_RTT`, keeping the nearest (`Member::replace_on`).
    pub placed_over_near: u32,
}

/// `Stats::raft_in`'s length: a minute of a follower's AppendEntries at B0-02's rate, about 8 a second.
const RAFT_IN: usize = 512;

impl Stats {
    /// A leader's apply, which frees the slot the next append takes.
    fn applied_at(&mut self, c: Clocks) {
        self.last_apply_at = c.raw;
    }

    /// A command admitted at `admitted` rode the entry appended at `c`: its queue time, split at the last apply. On
    /// this isolate's own clock, the one its admission time and `commitMs` read, so queue plus the entry's round adds
    /// up to commitMs. cut-trace: split on the member clock, which leads the isolate's own by up to seconds under
    /// load, B0-29 run 4's `queue:slot` p99 read 2,476 ms against 934 on one clock (`tests/m0/results/cut-trace.md`).
    fn queued(&mut self, admitted: u64, c: Clocks) {
        if self.queue_ms.len() >= SAMPLES_MAX {
            return;
        }
        self.queue_ms.push(c.raw.saturating_sub(admitted) as u32);
        let (slot, post) = queue_split(admitted, self.last_apply_at, c.raw);
        self.queue_slot_ms.push(slot);
        self.queue_post_ms.push(post);
    }

    pub fn raft_in(&mut self, at: u64, bytes: usize, body_ms: u64, work_ms: u64) {
        if bytes < 1024 {
            return;
        }
        self.raft_in.push_back([at, bytes as u64, body_ms, work_ms]);
        if self.raft_in.len() > RAFT_IN {
            self.raft_in.pop_front();
        }
    }

    /// A first attempt's answer (`crate::prsync::one`): its shard's budget counts, and its asks and sheds.
    pub fn test_merges(&mut self, v: &serde_json::Value) {
        if !v["budget"].is_null() {
            self.test_merges = Some(v["budget"].clone());
        }
        self.test_merge_asks += v["asked"].as_u64().unwrap_or(0) as u32;
        self.test_merge_shed += v["shed"].as_u64().unwrap_or(0) as u32;
    }

    /// Records one admitted command's priced bytes: `len` its `stored_len`, less `inline` when it queued as Agent, as
    /// `Admission::cmd_bytes` counts it.
    pub fn admitted_bytes(&mut self, class: raft::admit::AdmitClass, len: usize, inline: usize) {
        use raft::admit::AdmitClass as A;
        let (name, soft) = match class {
            A::Human => ("human", len),
            A::MergeQueue => ("mergeQueue", len),
            A::Bot => ("bot", len),
            A::Agent => ("agent", len.saturating_sub(inline)),
        };
        let v = self.admitted_bytes.entry(name).or_default();
        if v.len() < SAMPLES_MAX {
            v.push(soft.min(u32::MAX as usize) as u32);
        }
    }

    /// `/stats?reset=1`: drops the samples and counts a row reads back, and restarts the inbound clock.
    pub fn reset(&mut self) {
        self.commit_ms.clear();
        self.round_ms.clear();
        self.apply_ms.clear();
        self.apply_steps.clear();
        self.confirm_ms.clear();
        self.confirm_split.clear();
        self.big_tx.clear();
        self.append_ms.clear();
        self.raft_in.clear();
        self.deps_ms.clear();
        self.dep_fails.clear();
        self.dep_hedges = [0; 2];
        self.queue_ms.clear();
        self.queue_slot_ms.clear();
        self.queue_post_ms.clear();
        self.held_at_append.clear();
        self.cycle_ms.clear();
        self.last_append_at = None;
        self.admitted_bytes.clear();
        self.read_stages.clear();
        self.narrow_build_ms.clear();
        self.entries = 0;
        self.commands = 0;
        self.failed_calls = 0;
        self.test_merge_asks = 0;
        self.test_merge_shed = 0;
        self.peers.clear();
        self.calls.clear();
        self.trace.clear();
        self.hops.clear();
        self.inbound.clear();
        self.reads_by.clear();
        self.clock_lag = [0; 16];
        self.clock_lag_max = 0;
        self.lease_fallbacks = 0;
        self.inbound_since = now_ms();
    }
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
    /// A transaction's storage confirm (`sync()`) took `ms`; `index` is the applied index it covered.
    Confirm = 10,
    /// A learner's call to `peer` had no answer within `HEDGE`, and the same envelope was sent again; `index` is 1 if
    /// the copy was answered, `ms` its time.
    Hedge = 11,
    /// B0-02's leader trace (`tests/m0/results/carried-4.md`). An AppendEntries with entries to `peer` was handed to
    /// `call`; `batch` is its round, `index` its last entry, `ms` its bytes.
    AppendQueued = 12,
    /// That call's fetch was issued (its encoded body built and the stub's `fetch` called): `ms` since `AppendQueued`.
    AppendIssued = 13,
    /// That call ended: `index` 0 ok, 1 failed; `ms` since `AppendIssued`.
    AppendEnded = 14,
    /// An input that wrote: `ms` its synchronous step and persist, `index` the last entry it queued.
    Stepped = 15,
    /// The deferred transaction ran: `ms` how long after it was deferred, `index` its statements.
    Flushed = 16,
    /// An AppendEntries was stepped here: `index` the last entry it carried (its `prev_index` when empty), `ms` its
    /// commit, `batch` 0 when it rode the response to this member's own call, 1 when the leader sent it as a call
    /// (narrow-reads-3's split of a learner's apply wait).
    AppendStepped = 17,
    /// An AppendEntries for a learner calling this member was held for its next call's response (`crate::callers`):
    /// `batch` its round, `index` its last entry, `ms` its commit.
    AppendHeld = 18,
    /// A held AppendEntries left in the response to a learner's call: `batch` its round, `index` its last entry, `ms`
    /// its commit.
    AppendTaken = 19,
    /// A `/raft` call arrived here (narrow-reads-3's wire split): `peer` its sender, `index` the sender's clock when
    /// it issued the fetch (`x-graft-sent`), `ms` how long its body took to read. Logged when the handler starts, so
    /// against `index` it bounds the wire plus this isolate's input queue, and against `IndexTaken` the body and decode.
    #[cfg(feature = "test-faults")]
    CallArrived = 20,
    /// The answer to this member's `/raft` call to `peer` reached it: `index` the callee's clock when its handler
    /// returned the answer (`x-graft-left`), `ms` 0. Against `index` it bounds the output gate plus the wire back.
    #[cfg(feature = "test-faults")]
    AnswerArrived = 21,
    // 22 is `AppendAnswered` on the held far-leg merge (`m0-adaptive-append`).
    /// F74's call trace (`tests/m0/results/f74-trace.md`): this member's `/raft` fetch to `peer` is about to be issued.
    /// `at` is the `x-graft-sent` stamp, `batch` the call's id (`x-graft-call`, per host, so the callee's
    /// `CallArrived` and `CallLeft` and this member's `AnswerArrived` carry it too), `index` this member's `/raft`
    /// fetches then waiting for response headers, this one included, and `ms` its event-loop lag (`Host::loop_lag`).
    #[cfg(feature = "test-faults")]
    CallSent = 23,
    /// F74's call trace: this member's `/raft` handler for a call from `peer` returned. `at` is the `x-graft-left`
    /// stamp, `batch` the call's id, `index` the handler's entry (the top of `Routes::fetch`, before any injected
    /// fault awaits; `CallArrived` comes after them and the body), and `ms` this member's event-loop lag there.
    #[cfg(feature = "test-faults")]
    CallLeft = 24,
    /// F74's inbound trace (`tests/f74/inbound.py`): a request reached this member's handler, at its top. `at` the
    /// entry stamp (`CallLeft`'s `index` for a `/raft` call), `peer` the sender's address (`x-graft-from`, 255 for an
    /// edge or harness request), `batch` the call id for a `/raft` call (`x-graft-call`, else 0), `index` the arrival
    /// number on this host times 64 plus the route's code (`ROUTES`), and `ms` the requests open in this host's
    /// handler then, this one included.
    #[cfg(feature = "test-faults")]
    Entered = 25,
    /// m41-held's read source: a `/read` request's query, logged once its body is read. `batch` is its `Entered` row's
    /// arrival number (that row's `index` / 64), `index` the query's code in `READ_QUERIES` (`read_query_code`), and `ms` 1
    /// for a read-index read (`"index": true`, from a follower or learner pass) and 0 for one sent to the leader.
    #[cfg(feature = "test-faults")]
    ReadQuery = 26,
    /// probe-known's walk split: a request that carried `graftyard_lookup::SENT_BY`. `batch` is its `Entered` row's
    /// arrival number, `index` the sending isolate's id and `ms` the lookup step (`graftyard_lookup::SentBy`).
    #[cfg(feature = "test-faults")]
    SentBy = 27,
    /// colo-unknown's check: a `/suspect` report stepped here. `peer` is the member it is about, `batch` its term,
    /// `index` its colo (`raft::Colo::UNKNOWN` for a site script's lookup), and `ms` the verdict's code in
    /// `VERDICTS` (`crate::member`), so a trace splits reports by colo and sees which ones a promise answered.
    #[cfg(feature = "test-faults")]
    Reported = 28,
    /// cut-trace: the leader cut a batch for an entry (`raft::CutNote`, packed by `cut_row`). `peer` is the end code
    /// (`raft::CutEnd`) plus 16 × the frozen batches held before it plus 64 if an entry was in flight; `batch` the
    /// commands taken per class, 12 bits each in `AdmitClass::ALL` order; `index` the commands left queued per class,
    /// 13 bits each; `ms` the soft bytes, all bytes × 2^21 and the agent tokens left × 2^42. Each column stays under
    /// 2^53, so the JS number the trace passes through holds it exactly.
    #[cfg(feature = "test-faults")]
    Cut = 29,
}

/// `Trace::Cut`'s columns (`peer`, `batch`, `index`, `ms`) for one cut; `tests/m0/cuttrace/cuts.py` unpacks them.
#[cfg(any(test, feature = "test-faults"))]
fn cut_row(n: &raft::CutNote) -> (u8, u64, u64, u64) {
    let clamp = |v: u32, bits: u32| u64::from(v).min((1 << bits) - 1);
    let peer = n.end as u8 + 16 * n.held.min(3) as u8 + if n.in_flight { 64 } else { 0 };
    let taken = (0..4).fold(0, |a, i| a | clamp(n.taken[i], 12) << (12 * i));
    let left = (0..4).fold(0, |a, i| a | clamp(n.left[i], 13) << (13 * i));
    let bytes = |v: usize| (v as u64).min((1 << 21) - 1);
    let ms = bytes(n.soft_bytes) | bytes(n.bytes) << 21 | (n.tokens.max(0.0) as u64).min((1 << 11) - 1) << 42;
    (peer, taken, left, ms)
}

/// F74's inbound trace: route codes in `Trace::Entered`, code `i + 1` for `ROUTES[i]` and 0 for any other path.
/// Append-only, as trace kinds are; `tests/f74/inbound.py` keeps the same list.
#[cfg(any(test, feature = "test-faults"))]
pub const ROUTES: &[&str] = &[
    "/raft",
    "/read",
    "/commit",
    "/commit_many",
    "/outcome",
    "/outcome_by_request",
    "/suspect",
    "/campaign",
    "/status",
    "/stats",
    "/trace",
    "/ping",
    "/rtt",
    "/place",
    "/learners",
    "/record",
];

/// m41-held: query codes in `Trace::ReadQuery`, code `i + 1` for a query `read_query_code` names `READ_QUERIES[i]`, 0 for
/// any other. Append-only, as `ROUTES` is; `tests/f74/inbound.py` keeps the same list.
#[cfg(any(test, feature = "test-faults"))]
pub const READ_QUERIES: &[&str] = &[
    "probe",
    "dep",
    "rows",
    "prsync",
    "accountLogins",
    "user",
    "summary",
    "split",
    "settings",
    "request",
    "repoOwner",
    "repo",
    "ranges",
    "placement",
    "mergequeue",
    "login",
    "jti",
    "hosted",
    "genesis",
    "app",
    "key",
    "packed",
    "refs",
    "core",
    "lsRefs",
];

/// A `/read` body's query code (`READ_QUERIES`): what it selects, not its first key, because `serde_json` sorts a
/// query's keys and m42p's trace named the edge's core rows `ranges` and its whole-tier dumps `packed` or `other`
/// (`tests/am1/results/lm1-memory.md`, round 37). A `rows` read is `packed` for a whole-tier dump (`{"rows": true}`),
/// `core` for the read gate's access, agent, or actions rows, and `rows` for any other selection; `ranges` and
/// `packed` beside `rows` only shape its answer. Otherwise the query's first key, or `key` for the old shape.
#[cfg(any(test, feature = "test-faults"))]
pub fn read_query_code(body: &serde_json::Value) -> u64 {
    let name = match body.get("q") {
        Some(q) => match q.get("rows") {
            Some(serde_json::Value::Bool(_)) => Some("packed".to_owned()),
            Some(r) if ["access", "agent", "actions"].iter().any(|k| r.get(k).is_some()) => Some("core".to_owned()),
            Some(_) => Some("rows".to_owned()),
            None => q.as_object().and_then(|o| o.keys().next().cloned()),
        },
        None => body.get("key").map(|_| "key".to_owned()),
    };
    name.and_then(|k| READ_QUERIES.iter().position(|q| *q == k)).map_or(0, |i| i as u64 + 1)
}

/// F74: every request this host's handler takes, numbered in arrival order, and how many are open in it.
#[cfg(any(test, feature = "test-faults"))]
#[derive(Default)]
pub struct Inbound {
    seq: Cell<u64>,
    open: Rc<Cell<usize>>,
}

#[cfg(any(test, feature = "test-faults"))]
impl Inbound {
    /// A request to `path` arrived: its `Entered` row's `index` (arrival number times 64 plus the route's code) and
    /// `ms` (the requests open, this one included), and a guard that counts it open until the handler drops it.
    pub fn enter(&self, path: &str) -> (u64, u64, Counted) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let code = ROUTES.iter().position(|r| *r == path).map_or(0, |i| i as u64 + 1);
        let open = Counted::new(&self.open, 1);
        (seq * 64 + code, self.open.get() as u64, open)
    }
}

/// F74: how far behind this isolate's event loop reads at `now`, from the member's 100 ms ticker: the lateness of its
/// last wake, or how overdue its pending tick is, whichever is larger; 0 when the ticker is stopped (`next_due` 0).
/// Workers' clock advances only on I/O, so a turn spent in JavaScript is invisible until the next I/O's stamp, and a
/// lag here is a stall that pushed I/O past the tick's due time, not the CPU time of any one turn.
#[cfg(any(test, feature = "test-faults"))]
fn loop_lag(now: u64, last_late: u64, next_due: u64) -> u64 {
    if next_due == 0 { 0 } else { last_late.max(now.saturating_sub(next_due)) }
}

/// Trace entries kept per member: 16 learners' batches at 20 a second for a 30 s run, on the leader, twice over.
#[cfg(feature = "test-faults")]
const TRACE_MAX: usize = 60_000;

/// Test installs: the counted bytes at their peak (`Host::mark_mem`): call bodies in flight, read answers streaming,
/// learner holds and the log, their sum, when, and the isolate's wasm memory then. L0-12 m3fr3's new leaders stopped
/// answering `/stats` in the seconds before each memory reset, so a 2 s sampler saw none of those seconds.
#[cfg(feature = "test-faults")]
#[derive(Default, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemHigh {
    pub at: u64,
    pub total: usize,
    pub calls: usize,
    pub answers: usize,
    pub held: usize,
    pub log: usize,
    pub wasm_mb: f64,
}

/// A `mem high` log line once the counted bytes pass this, and again each MEM_LOG_STEP higher: Workers Logs keep
/// what a reset isolate's `/stats` loses.
#[cfg(feature = "test-faults")]
const MEM_LOG_FROM: usize = 16 << 20;
#[cfg(feature = "test-faults")]
const MEM_LOG_STEP: usize = 8 << 20;

/// Election trace rows kept per member: L0-12's ten kills, each a few dozen rows, many times over.
#[cfg(feature = "test-faults")]
const ELECT_MAX: usize = 2_000;

/// One election trace row's kind (`Stats::elections`), each seen by the host around a step (L0-12 m3fr2's 18 s and
/// 26 s leaderless spans, split into pre-vote, vote, and first commit).
#[cfg(any(test, feature = "test-faults"))]
#[derive(Clone, Copy)]
pub enum Elect {
    /// This member sent a vote request to `peer`: `extra` 1 for a pre-vote, `index` its last index, `term` the term
    /// asked for.
    VoteSent = 1,
    /// This member answered `peer`'s vote request: `term` the term asked for, `extra` 1 for a pre-vote, `index` 0 when
    /// granted, else its `Deny` code (`deny_code`).
    VoteAnswered = 2,
    /// The role changed in this step: `index` the old role, `extra` the new (`role_code`), `term` the term after.
    Role = 3,
    /// A leader's commit index first moved in its term: `index` the commit.
    FirstCommit = 4,
    /// A campaign ended: `extra` 1 won, 0 lost (CAMPAIGN_DEADLINE or a higher term).
    CampaignEnded = 5,
    /// A `/suspect` report (`peer` the member reported, `term` the reported term) or a `/campaign` request (`peer`
    /// 255, `term` 0): `extra` 1 when it started a pre-vote, else 0.
    #[cfg_attr(not(feature = "test-faults"), allow(dead_code))]
    Asked = 6,
}

#[cfg(any(test, feature = "test-faults"))]
fn role_code(r: Role) -> u64 {
    match r {
        Role::Follower => 0,
        Role::Candidate => 1,
        Role::Leader => 2,
    }
}

/// `Deny` as a number, 0 for none, in the enum's order.
#[cfg(any(test, feature = "test-faults"))]
fn deny_code(d: Option<raft::Deny>) -> u64 {
    use raft::Deny::*;
    match d {
        None => 0,
        Some(Stale) => 1,
        Some(NoConfig) => 2,
        Some(NotMember) => 3,
        Some(Leader) => 4,
        Some(Promised) => 5,
        Some(LogBehind) => 6,
        Some(Contending) => 7,
        Some(Quarantine) => 8,
        Some(Voted) => 9,
        Some(Drained) => 10,
        Some(Suspected) => 11,
    }
}

/// The election rows (`Elect`) of one step at `t`, from the role, term, and commit before (`was`) and after (`now`) and
/// what the step sent and resolved. `first` holds the term whose first leader commit was logged, so each term's is
/// logged once.
#[cfg(any(test, feature = "test-faults"))]
fn elect_rows<C: raft::Command>(
    was: (Role, raft::Term, Index),
    now: (Role, raft::Term, Index),
    out: &Out<C>,
    first: &Cell<u64>,
    t: u64,
) -> Vec<[u64; 6]> {
    let mut rows = Vec::new();
    let (role, term, commit) = now;
    if role != was.0 {
        rows.push([t, Elect::Role as u64, term.0, 0, role_code(was.0), role_code(role)]);
    }
    for e in out.early.iter().chain(&out.messages) {
        match &e.msg {
            raft::Message::Vote(r) => {
                rows.push([t, Elect::VoteSent as u64, r.term.0, u64::from(e.to.0), r.last_index.0, u64::from(r.pre)])
            }
            raft::Message::VoteRes(r) => rows.push([
                t,
                Elect::VoteAnswered as u64,
                r.asked.0,
                u64::from(e.to.0),
                deny_code(r.deny),
                u64::from(r.pre),
            ]),
            _ => {}
        }
    }
    for r in &out.resolutions {
        if let Resolution::Campaign(v @ (raft::Verdict::Won | raft::Verdict::Lost)) = r {
            rows.push([t, Elect::CampaignEnded as u64, term.0, 0, 0, u64::from(*v == raft::Verdict::Won)]);
        }
    }
    if role == Role::Leader && commit > was.2 && first.get() != term.0 {
        first.set(term.0);
        rows.push([t, Elect::FirstCommit as u64, term.0, 0, commit.0, 0]);
    }
    rows
}

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
    /// Every call to this peer that ended, and their encoded bytes: a leader's egress per member (narrow-reads-3,
    /// learners pipelined).
    pub sent: u32,
    pub sent_bytes: u64,
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

/// How long a learner's call waits for its answer before the same envelope is sent once more (`Host::hedge`)
/// [guess]: past the slowest answer an apac learner had from an enam leader in a normal second of B0-18 (p99 330 ms,
/// `tests/m3/results/m3-wp5.md`), and half of CALL_DEADLINE.
const HEDGE: Duration = Duration::from_millis(500);

pub struct Core<F: Family> {
    pub m: raft::Member<F::C>,
    pub join: Join,
    applied: Index,
    waiters: HashMap<Ticket, Vec<oneshot::Sender<Outcome<F>>>>,
    /// The request id and admission time of each ticket this boot admitted, for kills and latency, and what the
    /// ticket needs at resolution beyond them: `F::marked`'s slim command for the marker puts after an `Acked`, or,
    /// once `decide` refused it, the core's own command (`Out::refused`) for the id-block lease loop's re-send. A
    /// whole copy of every admitted command held 60 to 80 KB a ticket on B0-33's leaders (LM-1 round 19,
    /// `tests/m0/results/admitted-bytes.md`).
    pub admitted: HashMap<Ticket, (RequestId, u64, Option<F::C>)>,
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
    /// Each dependency read's per-chain record by its id, until `DepsUsed` names the entry that used it (`deps_log`).
    #[cfg(feature = "test-faults")]
    pub dep_reads: HashMap<u64, Vec<Value>>,
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
    /// When this member last took a call an edge made for a client (`graftyard_lookup::CLIENT_CALL`); 0 in a fresh
    /// isolate. The daily near check runs only within DRAIN_IDLE_MS of it, so a cron, audit, sweep, or peer call never
    /// starts one.
    pub client_at: Cell<u64>,
    /// While this voter has leader-only work and hears no leader: when it first found so, and when it last acted
    /// (`keep_led`); (0, 0) otherwise.
    unheard: Cell<(u64, u64)>,
    /// Whole-tier reads in flight on this member (`crate::member::HEAVY_READS`).
    pub heavy_reads: Rc<Cell<u32>>,
    /// Reads of any kind in flight on this member, until their answer is sent (`crate::member::READS_MAX`).
    pub reads_in_flight: Rc<Cell<u32>>,
    /// This member's clock: the isolate's, raised to the freshest stamp a peer or the edge sent (`crate::clock`).
    /// Every input steps at it, and every reading the member gives out is it.
    pub clock: RefCell<crate::clock::Clock>,
    /// Bytes of the bodies of member calls this member has sent and not yet seen end, and of read answers still
    /// streaming, each counted once per call or read though a shared answer is one copy (`/stats` `memSplit`).
    pub call_bytes: Rc<Cell<usize>>,
    /// Distinct bytes of read answers still streaming: a shared answer once, however many reads stream it
    /// (`crate::member::ANSWER_BYTES_MAX`).
    pub answers: Rc<RefCell<Answers>>,
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
    /// Test installs: the term whose first leader commit has an `Elect::FirstCommit` row.
    #[cfg(feature = "test-faults")]
    first_commit: Cell<u64>,
    /// Truncations stepped, numbered, with the index each left the log at.
    truncs: RefCell<VecDeque<(u64, u64)>>,
    /// Commands refused `NeedIdBlock`, waiting for the lease loop: request id, command, and their waiters.
    blocked: RefCell<Vec<(Ticket, RequestId, F::C)>>,
    leasing: Cell<bool>,
    /// Messages for learners that are calling this member, held for their next call (`crate::callers`).
    pub callers: RefCell<crate::callers::Callers<Envelope<F>>>,
    /// The highest commit index a peer told this member this boot (`heard`): a member behind its leader's appends
    /// commits only what it holds, and the feed tells a cursor the group committed from one it never did by this.
    heard_commit: Cell<u64>,
    /// Transfers to abandon: their `TimeoutNow` is dropped (`/transfer` with `abandon`).
    #[cfg(feature = "test-faults")]
    pub abandon: RefCell<BTreeSet<u64>>,
    /// Test installs (LF-13): until this time (epoch ms) this member fails as a site fault fails a whole member class:
    /// it answers every call but the observation routes with 503 and fails each call it sends before it leaves
    /// (`/sever`). Set and lifted in this object at once, with no plan cache in between.
    #[cfg(feature = "test-faults")]
    pub severed_until: Cell<u64>,
    /// Test installs, B0-02's A/B (`tests/m0/results/carried-5.md`): member calls send their body as one array, as
    /// before `m0-carried-4`, instead of a stream (`/stats?body=array`). A reset object streams again.
    #[cfg(feature = "test-faults")]
    pub array_body: Cell<bool>,
    /// Test installs, B0-02 (`tests/m0/results/carried-6.md`): BF-14's `arrival` rows kept in memory instead of
    /// written to SQLite (`/stats?arrival=mem`), so no write a production member does not make sits before the
    /// leader's next AppendEntries. `/arrivals` serves them either way. A reset object writes rows again.
    #[cfg(feature = "test-faults")]
    pub arrivals_mem: RefCell<Option<VecDeque<Value>>>,
    /// Test installs, B0-02's cap A/B (`tests/m0/results/carried-7.md`): where the leader stops filling an entry
    /// (`/stats?entry=max` for ENTRY_MAX, `entry=soft` for ENTRY_SOFT, `entry=<bytes>`). A reset object caps at
    /// ENTRY_SOFT again.
    #[cfg(feature = "test-faults")]
    pub entry_soft: Cell<usize>,
    /// Test installs (F67): until this time (epoch ms) every answer to this member's AppendEntries is lost after the
    /// follower ran it, so its appends still renew the followers' promises while it commits nothing: the
    /// simulator's `HalfHung` on one member (`/sever` with `half`).
    #[cfg(feature = "test-faults")]
    pub half_hung_until: Cell<u64>,
    /// Test installs, B0-18's depth model: the hop of the newest request this object received, as its sender's
    /// `x-graft-hop` header said, or 2 for a request from a Worker (the edge, the tools edge). The runtime charges
    /// every outgoing call to the newest incoming request, so a call leaves at this hop plus one.
    #[cfg(feature = "test-faults")]
    pub hop: Cell<u64>,
    /// B0-07: the sites this member last told the core may not lead (`crate::faults::SiteModes`), so `tick` steps
    /// `set_drained` only when they change.
    #[cfg(feature = "test-faults")]
    drained: RefCell<Option<(String, BTreeSet<u8>)>>,
    /// F74: the last `/raft` call id this host issued (`Trace::CallSent`). It starts at the host's build time in ms
    /// times 1,000, so ids stay unique across the object's resets and under 2^53, where a JSON trace keeps them exact.
    #[cfg(feature = "test-faults")]
    call_seq: Cell<u64>,
    /// F74: this host's `/raft` fetches waiting for response headers.
    #[cfg(feature = "test-faults")]
    awaiting: Rc<Cell<usize>>,
    /// F74: the ticker's last wake lateness and its pending tick's due time, 0 when stopped (`loop_lag`).
    #[cfg(feature = "test-faults")]
    tick_lag: Rc<Cell<(u64, u64)>>,
    /// F74: the inbound trace's arrival numbers and open requests (`Trace::Entered`).
    #[cfg(feature = "test-faults")]
    pub inbound_log: Inbound,
}

/// `n` bytes counted in a gauge for as long as this lives.
pub struct Counted(Rc<Cell<usize>>, usize);

impl Counted {
    pub fn new(gauge: &Rc<Cell<usize>>, n: usize) -> Counted {
        gauge.set(gauge.get() + n);
        Counted(gauge.clone(), n)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(self.1));
    }
}

/// Reports an AppendEntries call ended when the task that awaited it ends, by any path (`Member::call_ended`).
struct Ended<F: Family>(Rc<Host<F>>, Option<(SiteId, u64)>);

impl<F: Family> Drop for Ended<F> {
    fn drop(&mut self) {
        if let Some((to, round)) = self.1 {
            let _ = self.0.input(|m, t| m.call_ended(t, to, round));
        }
    }
}

/// The read answers streaming out of this member, by the address of their shared bytes, with how many reads stream
/// each, and their distinct bytes. An answer's address is unique while a hold keeps it alive.
#[derive(Default)]
pub struct Answers {
    by: HashMap<usize, u32>,
    bytes: usize,
}

impl Answers {
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Counts `v` while the returned hold lives: its bytes once, however many reads hold it.
    pub fn hold(this: &Rc<RefCell<Answers>>, v: &Rc<[u8]>) -> AnswerHeld {
        let key = v.as_ptr() as usize;
        let mut a = this.borrow_mut();
        let n = a.by.entry(key).or_insert(0);
        *n += 1;
        if *n == 1 {
            a.bytes += v.len();
        }
        AnswerHeld(this.clone(), key, v.len())
    }
}

pub struct AnswerHeld(Rc<RefCell<Answers>>, usize, usize);

impl Drop for AnswerHeld {
    fn drop(&mut self) {
        let mut a = self.0.borrow_mut();
        if let Some(n) = a.by.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                a.by.remove(&self.1);
                a.bytes -= self.2;
            }
        }
    }
}

pub fn err(m: impl Into<String>) -> Error {
    Error::RustError(m.into())
}

/// Background writes of a leader's own (the split's `MetaSlim`, the archive's `Promoted` and `Archived`) wait `PACE`
/// times as long as each entry took to commit before the next, at most `PACE_MAX`, so followers and the leader spend
/// at most about a third of their time applying them. Back to back, one `MetaSlim` every 0.45 s held each follower's
/// `/raft` at 400 to 550 ms with 100 to 200 ms of CPU, its other calls queued behind them to about 3 s, and a leader's
/// 20 s slim run took up to 7.5 s of CPU: about 110 `STEP_DOWN` trips on `CommitErrors` and `LoopLag` in 2.5 h
/// [measured, `tests/am1/results/b030-split.md`, "Smoke 1"; the third is an inference].
pub const PACE: u32 = 2;
pub const PACE_MAX: Duration = Duration::from_secs(10);

/// The wait after a paced write whose entry took `took_ms` to commit (`PACE`).
pub fn paced(took_ms: u64) -> Duration {
    (Duration::from_millis(took_ms) * PACE).min(PACE_MAX)
}

/// A voter whose group has leader-only background work (`Family::needs_leader`) acts as one edge `suspect` report
/// would make it once it has seen no leader for 2 x ELECTION_AFTER, the design's bound for waiving the reset rule
/// (`design/DESIGN.md`, Shape, `suspect`, and "Downtime", Gray region): its promise, given for ELECTION_AFTER from the
/// leader's last append, lapsed ELECTION_AFTER ago. It acts again at most every ELECTION_AFTER. Campaigns are
/// otherwise request-driven, and a split's slim pass that no client called left its group leaderless for 39 minutes
/// once its leader stopped leading (`tests/am1/results/b030-split.md`, "Smoke 1").
pub const LEADERLESS_FOR: Duration = raft::ELECTION_AFTER;
/// How often a member with leader-only work due checks, and how soon after a probe.
const KEEP_LED_EVERY: i64 = 5_000;
const PROBED_CHECK: i64 = 1_000;

/// What `Host::keep_led` does on one alarm.
#[derive(Debug, PartialEq, Eq)]
pub enum KeepLed {
    /// It leads or its promise to a leader runs, or no leader-only work is due.
    Off,
    /// Unheard, not yet for the bound.
    Wait,
    /// Unheard for the bound, and it still names a leader: ask that leader for a read index. A leader that leads
    /// answers, and nothing else happens (an idle leader sends no append, so a lapsed promise alone is no evidence).
    /// One that does not lead answers none and is forgotten, and one that does not answer in CAMPAIGN_DEADLINE is
    /// forgotten and the member campaigns (F67's read-index rule).
    Probe,
    /// Unheard for the bound and naming no leader: campaign.
    Campaign,
}

/// `keep_led`'s rule. `heard`: it leads, or its promise to a leader runs. `named`: it names a leader other than
/// itself. `needs`: its group has leader-only work due. `lapsed`: when its last promise lapsed (`promise_until`, in
/// its stored state, so a reset keeps it), 0 for none. `found`: when it was first found unheard, for a member that
/// never promised. `acted`: when it last probed or campaigned, 0 for not since it last heard a leader.
pub fn keep_led(heard: bool, named: bool, needs: bool, lapsed: u64, found: u64, acted: u64, now: u64) -> KeepLed {
    if heard || !needs {
        return KeepLed::Off;
    }
    let base = if lapsed > 0 { lapsed } else { found }.max(acted);
    if now < base + LEADERLESS_FOR.as_millis() as u64 {
        KeepLed::Wait
    } else if named {
        KeepLed::Probe
    } else {
        KeepLed::Campaign
    }
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

/// Whether a storage error is the object out of space: SQLite's own `SQLITE_FULL`, or the platform's database size
/// limit, which is what a member hit on B0-30's short run ("Exceeded the maximum database size", site c at 10.70 GB)
/// while its stats still read 0.
pub fn out_of_space(e: &str) -> bool {
    ["SQLITE_FULL", "database or disk is full", "Exceeded the maximum database size"].iter().any(|m| e.contains(m))
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
                    for a in F::archived(e) {
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
                        // A request id already on disk is the double apply BF-12 counts: on test installs the
                        // transaction writes its `dup` row only then (`crate::txn`).
                        self.stmts.push((
                            crate::txn::REQUEST_INSERT,
                            vec![rid.to_string().into(), i(idx), i(e.at.0), b.into()],
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

pub use crate::txn::commit_tx;

/// Rows one at a time: `rows` copies every row of a result into wasm memory first, and a log of 1 MB inline packs read
/// that way ran the isolate out of memory in `load` (F45, `tests/m1/results/m1-f43.md`). A read stopped early still
/// ends its statement (`crate::logread`).
use crate::logread::each_row;

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
    /// The cause of the first read SQLite refused or row that did not decode, as `TierSql::lost` keeps it: the read
    /// answered no row, and the host resets this member before anything computed from it leaves (`Host::lost_cause`).
    lost: Option<String>,
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
            inner: Rc::new(RefCell::new(ReqInner {
                sql,
                pending: BTreeMap::new(),
                cut: Timestamp(0),
                row_event,
                lost: None,
            })),
        }
    }

    /// Why a read of the table failed, if one did since the member was built.
    pub fn lost(&self) -> Option<String> {
        self.inner.borrow().lost.clone()
    }

    /// A refused read: logged with its cause and kept for the host (`RequestSql::lost`). The caller answers no row.
    fn refused(&self, why: String) {
        console_error!("request table lost: {why}");
        self.inner.borrow_mut().lost.get_or_insert(why);
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
        let r = match rows(&sql, "SELECT idx, at, events FROM request WHERE rid = ?", vec![id.to_string().into()]) {
            Ok(r) => r,
            Err(e) => {
                self.refused(format!("read of {id}: {e}"));
                return None;
            }
        };
        let row = match self.decode(id, r.first()?) {
            Ok(row) => row,
            Err(e) => {
                self.refused(e.to_string());
                return None;
            }
        };
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
        let read = each_row(&sql, "SELECT rid, idx, at, events FROM request", vec![], |r| {
            let rid = rid_from(text(&r[0])?)?;
            let row = self.decode(rid, &r[1..])?;
            if row.at >= cut {
                out.insert(rid, row);
            }
            Ok(true)
        });
        if let Err(e) = read {
            self.refused(format!("read of every row: {e}"));
        }
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
    stored_entry_sized::<F>(sql, at).map(|(e, _)| e)
}

/// `stored_entry` with its row's bytes, for a reader that bounds what one alarm reads (`graftyard_git::prsync::scan`).
pub fn stored_entry_sized<F: Family>(sql: &SqlStorage, at: Index) -> Option<(Entry<F>, usize)> {
    let r = rows(sql, "SELECT row FROM log WHERE idx = ?", vec![i(at.0)]).ok()?;
    let row = blob(&r.first()?[0]).ok()?;
    Some((codec::from_bytes(row).ok()?, row.len()))
}

fn stored_range<F: Family>(sql: &SqlStorage, from: Index, through: Index) -> Option<Vec<Entry<F>>> {
    let lo = from.0.max(1);
    let mut entries: Vec<Entry<F>> = Vec::new();
    let (mut bytes, mut whole) = (0usize, true);
    // In pages read to their end (`crate::logread::log_after`): the read stops at `APPEND_BYTES_MAX`, and stepping
    // through up to `CATCHUP_MAX` rows of up to `ENTRY_MAX` only to end the statement would cost more than the read.
    let mut each = |r: Vec<SqlStorageValue>| -> Result<bool> {
        let idx = int(&r[0])? as u64;
        if idx > through.0 {
            return Ok(false);
        }
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
    };
    for r in crate::logread::log_after(sql, "length(row), row", lo - 1) {
        if !each(r.ok()?).ok()? {
            break;
        }
    }
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
            client_at: Cell::new(0),
            unheard: Cell::new((0, 0)),
            heavy_reads: Rc::new(Cell::new(0)),
            reads_in_flight: Rc::new(Cell::new(0)),
            clock: RefCell::new(crate::clock::Clock::default()),
            call_bytes: Rc::new(Cell::new(0)),
            answers: Rc::new(RefCell::new(Answers::default())),
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
            #[cfg(feature = "test-faults")]
            first_commit: Cell::new(0),
            truncs: RefCell::new(VecDeque::new()),
            blocked: RefCell::new(Vec::new()),
            leasing: Cell::new(false),
            callers: RefCell::new(Default::default()),
            heard_commit: Cell::new(0),
            #[cfg(feature = "test-faults")]
            abandon: RefCell::new(BTreeSet::new()),
            #[cfg(feature = "test-faults")]
            severed_until: Cell::new(0),
            #[cfg(feature = "test-faults")]
            array_body: Cell::new(false),
            #[cfg(feature = "test-faults")]
            arrivals_mem: RefCell::new(None),
            #[cfg(feature = "test-faults")]
            entry_soft: Cell::new(raft::ENTRY_SOFT),
            #[cfg(feature = "test-faults")]
            half_hung_until: Cell::new(0),
            #[cfg(feature = "test-faults")]
            hop: Cell::new(2),
            #[cfg(feature = "test-faults")]
            drained: RefCell::new(None),
            #[cfg(feature = "test-faults")]
            call_seq: Cell::new(now_ms() * 1_000),
            #[cfg(feature = "test-faults")]
            awaiting: Rc::new(Cell::new(0)),
            #[cfg(feature = "test-faults")]
            tick_lag: Rc::new(Cell::new((0, 0))),
            #[cfg(feature = "test-faults")]
            inbound_log: Inbound::default(),
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
    pub fn trace(&self, kind: Trace, peer: u8, batch: u64, index: u64, ms: u64) {
        self.trace_at(now_ms(), kind, peer, batch, index, ms);
    }

    /// Test installs: takes the counted bytes now into `Stats::mem_high` when they are the most since it was read,
    /// and logs `mem high` each MEM_LOG_STEP past MEM_LOG_FROM. Called where they grow: a call body or read answer
    /// counted, a message held for a learner, an input stepped.
    #[cfg(feature = "test-faults")]
    pub fn mark_mem(&self) {
        let log = self.core.try_borrow().ok().and_then(|c| c.as_ref().map(|c| c.m.log_bytes())).unwrap_or(0);
        let held = self.callers.try_borrow().map(|c| c.held_total()).unwrap_or(0);
        let (calls, answers) = (self.call_bytes.get(), self.answers.try_borrow().map(|a| a.bytes()).unwrap_or(0));
        let total = calls + answers + held + log;
        let Ok(mut s) = self.stats.try_borrow_mut() else { return };
        if total > s.mem_high.total {
            s.mem_high =
                MemHigh { at: now_ms(), total, calls, answers, held, log, wasm_mb: crate::load::wasm_mem_mb() };
        }
        if total < MEM_LOG_FROM {
            s.mem_logged = 0;
        } else if total >= s.mem_logged + MEM_LOG_STEP {
            s.mem_logged = total;
            let h = MemHigh { at: now_ms(), total, calls, answers, held, log, wasm_mb: crate::load::wasm_mem_mb() };
            console_log!("mem high {}", serde_json::to_string(&h).unwrap_or_default());
        }
    }

    /// Test installs: one `Stats::trace` entry, timed `at`.
    #[cfg(feature = "test-faults")]
    pub fn trace_at(&self, at: u64, kind: Trace, peer: u8, batch: u64, index: u64, ms: u64) {
        let mut s = self.stats.borrow_mut();
        if s.trace.len() >= TRACE_MAX {
            s.trace.pop_front();
        }
        s.trace.push_back([at, kind as u64, u64::from(peer), batch, index, ms]);
    }

    #[cfg(not(feature = "test-faults"))]
    pub fn trace_at(&self, _: u64, _: Trace, _: u8, _: u64, _: u64, _: u64) {}

    /// F74: this isolate's event-loop lag now (`loop_lag`).
    #[cfg(feature = "test-faults")]
    pub fn loop_lag(&self) -> u64 {
        let (late, due) = self.tick_lag.get();
        loop_lag(now_ms(), late, due)
    }

    /// Test installs: the election rows of one step (`elect_rows`), kept in `Stats::elections`.
    #[cfg(feature = "test-faults")]
    fn note_elect(&self, m: &raft::Member<F::C>, was: (Role, raft::Term, Index), out: &Out<F::C>, t: u64) {
        let now = (m.role(), m.term(), m.state().commit);
        let rows = elect_rows(was, now, out, &self.first_commit, t);
        if !rows.is_empty() {
            let mut s = self.stats.borrow_mut();
            for r in rows {
                if s.elections.len() >= ELECT_MAX {
                    s.elections.pop_front();
                }
                s.elections.push_back(r);
            }
        }
    }

    /// Test installs: an `Elect::Asked` row for a `/suspect` report about `about` (255 for `/campaign`).
    #[cfg(feature = "test-faults")]
    pub fn elect_asked(&self, about: u8, term: u64, started: bool) {
        let mut s = self.stats.borrow_mut();
        if s.elections.len() >= ELECT_MAX {
            s.elections.pop_front();
        }
        let t = now_ms();
        s.elections.push_back([t, Elect::Asked as u64, term, u64::from(about), 0, u64::from(started)]);
    }

    /// Raises `heard_commit` from a message a peer sent this member: an AppendEntries' `commit`, a read index, or an
    /// install's `through`, each an index the group committed.
    pub fn heard<E>(&self, e: &raft::Envelope<E>) {
        let i = match &e.msg {
            raft::Message::Append(a) => a.commit.0,
            raft::Message::ReadIndexRes { index: Some((i, _)), .. } => i.0,
            raft::Message::Install(s) => s.source.through.0,
            _ => return,
        };
        self.heard_commit.set(self.heard_commit.get().max(i));
    }

    /// The highest index this member knows the group committed: its own commit, or one its leader sent it.
    pub fn committed(&self, own: raft::Index) -> raft::Index {
        raft::Index(self.heard_commit.get().max(own.0))
    }

    /// Test installs: traces a `ReadIndexRes` stepped here, `own` when it answered this member's own call.
    pub fn trace_res<E>(&self, e: &raft::Envelope<E>, own: bool) {
        match &e.msg {
            raft::Message::ReadIndexRes { batch, index } => {
                self.trace(Trace::ResStepped, e.from.0, *batch, index.map_or(0, |(i, _)| i.0), u64::from(!own))
            }
            raft::Message::Append(a) => {
                let last = a.entries.last().map_or(a.prev_index.0, |x| x.index.0);
                self.trace(Trace::AppendStepped, e.from.0, u64::from(!own), last, a.commit.0)
            }
            _ => {}
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
        let retention = request_retention(&self.env, F::tiered());
        let (stored, requests) = load::<F>(&self.state.storage(), &join, retention)?;
        *self.requests.borrow_mut() = Some(requests);
        let applied = stored.state.as_ref().map_or(Index(0), |s| s.applied);
        // Everything `load` read back is durable.
        *self.synced.borrow_mut() = VecDeque::from([(stored.base.index.0 + stored.log.len() as u64, now_ms())]);
        let (mut m, ready) =
            raft::Member::<F::C>::restore(member_config::<F>(&site, &join), stored, Timestamp(self.now()), seed())
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
        #[cfg(feature = "test-faults")]
        m.set_entry_soft(self.entry_soft.get());
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
            #[cfg(feature = "test-faults")]
            dep_reads: HashMap::new(),
        });
        self.stats.borrow_mut().restores += 1;
        F::restored(self);
        if F::needs_leader(self) {
            self.arm_in(KEEP_LED_EVERY);
        }
        Ok(())
    }

    /// Inside an injected `down_ms` window after a kill: the object answers nothing but `/status`.
    #[cfg(feature = "test-faults")]
    pub fn down(&self) -> bool {
        let until = self.down_until.get();
        until != 0 && now_ms() < until
    }

    /// Inside a `/sever` window (LF-13).
    #[cfg(feature = "test-faults")]
    pub fn severed(&self) -> bool {
        now_ms() < self.severed_until.get()
    }

    pub fn joined(&self) -> bool {
        self.core.borrow().is_some()
    }

    /// The chain this member was joined to, if it is joined.
    #[cfg(feature = "test-faults")]
    pub fn chain(&self) -> Option<graftyard_types::ChainId> {
        self.core.try_borrow().ok()?.as_ref().map(|c| c.join.chain)
    }

    fn note(&self, core: &Core<F>, writes: &[Write<F::Ev>], c: Clocks) {
        let t = c.member;
        let mut s = self.stats.borrow_mut();
        let (leading, term) = (core.m.role() == Role::Leader, core.m.term());
        // An apply in the same input as an append freed the slot that append took.
        if leading && writes.iter().any(|w| matches!(w, Write::Apply(_))) {
            s.applied_at(c);
        }
        for w in writes {
            match w {
                Write::Append(e) if leading && e.term == term && matches!(e.body, EntryBody::Events(_)) => {
                    s.entries += 1;
                    s.commands += e.requests.len() as u32;
                    if s.cycle_ms.len() < SAMPLES_MAX {
                        if let Some((_, at)) = s.last_append_at.filter(|(tm, _)| *tm == term.0) {
                            s.cycle_ms.push(t.saturating_sub(at) as u32);
                        }
                        s.held_at_append.push(core.m.admission().map_or(0, |a| a.held) as u32);
                    }
                    s.last_append_at = Some((term.0, t));
                    s.appended.push((e.index.0, t));
                    for (rid, _) in &e.requests {
                        if let Some((_, at, _)) = core.admitted.values().find(|(r, _, _)| r == rid) {
                            s.queued(*at, c);
                        }
                    }
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

    /// This member's clock (`crate::clock::Clock::now`): every input steps at it, and every reading the member gives
    /// out, such as `/status`'s `now`, is it.
    pub fn now(&self) -> u64 {
        self.clock.borrow_mut().now(now_ms())
    }

    /// A sender's clock stamp (`crate::clock::STAMP`) on a call to this member or a reply to its call.
    pub fn stamped(&self, v: Option<String>) {
        if let Some(stamp) = crate::clock::parse(v) {
            let lag = self.clock.borrow_mut().stamp(stamp, now_ms());
            let mut s = self.stats.borrow_mut();
            s.clock_lag[(u64::BITS - lag.leading_zeros()).min(15) as usize] += 1;
            s.clock_lag_max = s.clock_lag_max.max(lag);
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
        let t = self.now();
        let applied0 = core.applied;
        #[cfg(feature = "test-faults")]
        let was = (core.m.role(), core.m.term(), core.m.state().commit);
        // The read lease rests on this clock: while stamps showed it lagging, reads wait for a round (`crate::clock`).
        core.m.set_clock_suspect(self.clock.borrow().suspect());
        core.m.set_own_clock(Timestamp(now_ms()));
        let ready = f(&mut core.m, Timestamp(t));
        self.note(core, &ready.writes, Clocks { member: t, raw: now_ms() });
        #[cfg(feature = "test-faults")]
        let appended = self.appended_kill(core, &ready.writes);
        let wrote = !ready.writes.is_empty();
        #[cfg(feature = "test-faults")]
        let ready_appended_events =
            ready.writes.iter().any(|w| matches!(w, Write::Append(e) if matches!(e.body, EntryBody::Events(_))));
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
        #[cfg(feature = "test-faults")]
        self.note_elect(&core.m, was, &out, t);
        #[cfg(feature = "test-faults")]
        for n in &out.cuts {
            let (peer, batch, index, ms) = cut_row(n);
            self.trace(Trace::Cut, peer, batch, index, ms);
        }
        let (applied, stmts, archived) = (store.applied, store.stmts, store.archived);
        if let Some(b) = store.installed {
            self.queued_last.set(Some(b.index.0));
            *self.synced.borrow_mut() = VecDeque::new();
        }
        // A fold read storage refused during `decide` or `apply`: nothing this input computed may leave.
        if let Some(why) = self.lost_cause(&core.m) {
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
        F::stepped(&core.m, &core.join, applied0, applied, &out);
        core.applied = applied;
        let early = std::mem::take(&mut out.early);
        if wrote {
            let id = core.m.confirm_begin(Timestamp(self.now()));
            let timed = core.m.role() == Role::Leader
                && early
                    .iter()
                    .chain(&out.messages)
                    .any(|e| matches!(&e.msg, raft::Message::Append(a) if !a.entries.is_empty()));
            self.unconfirmed.borrow_mut().push((id, timed.then(now_ms), out.self_ack.take()));
        }
        #[cfg(feature = "test-faults")]
        let lease_at = core.m.role() == Role::Leader && ready_appended_events;
        drop(guard);
        #[cfg(feature = "test-faults")]
        if wrote {
            self.mark_mem();
        }
        #[cfg(feature = "test-faults")]
        if lease_at {
            let sent = now_ms().saturating_sub(t) as u32;
            let h = self.clone();
            spawn_local(async move {
                Delay::from(Duration::ZERO).await;
                let next = now_ms().saturating_sub(t) as u32;
                let mut s = h.stats.borrow_mut();
                if s.lease_clock.len() < SAMPLES_MAX {
                    s.lease_clock.push([sent, next]);
                }
            });
        }
        if wrote {
            self.trace(Trace::Stepped, 0, 0, self.queued_last.get().unwrap_or(0), now_ms().saturating_sub(t));
        }
        self.queue.borrow_mut().extend(stmts);
        let defer = !early.is_empty() && wrote;
        // The leader's AppendEntries leave now. Each `send` issues its fetch in a microtask, before the deferred
        // transaction's macrotask, so the output gate does not hold the round behind this input's own write (Raft
        // dissertation 10.2.1, and Milestone 0's prototype). It does hold it behind an earlier input's transaction
        // that has not confirmed: the gate waits for "all previous writes" (Durable Objects storage API docs,
        // `allowUnconfirmed`; `tests/m0/results/carried-6.md`). The leader counts its own copy only once its
        // `SelfAck` comes back after `sync()`.
        for e in early {
            self.send(e);
        }
        if defer || self.flush_due.get() {
            self.defer_flush();
        } else if let Err(e) = self.flush() {
            self.full(&e);
            self.fail();
            return Err(err(format!("storage write failed: {e}")));
        }
        // In order, one task for all: an entry records up to `ARCHIVE_PIECES` ranges, and each truncates only once its
        // own key is in two buckets.
        if !archived.is_empty() {
            let h = self.clone();
            spawn_local(async move {
                for (key, to) in archived {
                    if !h.truncate(&key, to).await {
                        break;
                    }
                }
            });
        }
        Ok((r, out))
    }

    /// After an `Archived` through `to` applied: once its key is in two buckets, the stored log keeps no entry below
    /// `to` (`design/DESIGN.md`, Shape, "Packs": "followers truncate only through a committed range whose key they
    /// can HEAD in two buckets"). The row at `to` stays as the stored log's base: its term and digest anchor the next
    /// entry, and restore starts above it. Entries through `to` are archived either way (`RepoFold::first_index`).
    /// A key in fewer than two buckets truncates nothing and raises an alarm; re-putting the copy is not built.
    /// Whether the log was truncated.
    async fn truncate(self: &Rc<Self>, key: &str, to: u64) -> bool {
        self.settled().await;
        let Ok(site) = SiteEnv::read(&self.env) else { return false };
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
            return false;
        }
        match self.state.storage().sql().exec("DELETE FROM log WHERE idx < ?", vec![i(to)]) {
            Ok(_) => self.stats.borrow_mut().truncated_to = to,
            Err(e) => {
                console_error!("truncate below {to}: {e}");
                return false;
            }
        }
        F::truncated(self, Index(to)).await;
        true
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
                out.refused.iter().filter(|(i, _, _)| *i == e.index).map(|(_, r, _)| r.to_string()).collect();
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
            self.full(&e);
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
        let deferred = now_ms();
        spawn_local(async move {
            Delay::from(Duration::ZERO).await;
            if h.dying.get() {
                return;
            }
            let n = h.queue.borrow().len() as u64;
            h.trace(Trace::Flushed, 0, 0, n, now_ms().saturating_sub(deferred));
            if let Err(e) = h.flush() {
                console_error!("deferred storage write failed: {e}");
                h.full(&e);
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

    /// The statements queued for the next transaction, and roughly their bytes (`/stats` `memSplit`).
    pub fn queued(&self) -> (usize, usize) {
        let q = self.queue.borrow();
        let val = |v: &SqlStorageValue| match v {
            SqlStorageValue::String(t) => t.len(),
            SqlStorageValue::Blob(b) => b.len(),
            _ => 8,
        };
        (q.len(), q.iter().map(|(sql, args)| sql.len() + args.iter().map(val).sum::<usize>()).sum())
    }

    /// Runs every queued statement in one transaction, releases the outputs that waited for it, and reports the
    /// storage confirm, and each `SelfAck`, once `sync()` says the writes are durable. The Durable Object confirms
    /// writes in order, so one `sync()` covers every input queued so far.
    fn flush(self: &Rc<Self>) -> std::result::Result<(), String> {
        let stmts = std::mem::take(&mut *self.queue.borrow_mut());
        let (n_stmts, n_bytes) = stmt_bytes(&stmts);
        if n_bytes >= CONFIRM_BIG {
            let mut s = self.stats.borrow_mut();
            if s.big_tx.len() >= BIG_KEPT {
                s.big_tx.remove(0);
            }
            s.big_tx.push((n_stmts, n_bytes, stmt_mix(&stmts)));
        }
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
            let began = now_ms();
            // `ConfirmSplit::turn`: a zero-delay timer set beside the `sync()`, whose firing time is the turn queue.
            let turned = Rc::new(Cell::new(None));
            if pending.iter().any(|p| p.1.is_some()) {
                let turned = turned.clone();
                spawn_local(async move {
                    Delay::from(Duration::ZERO).await;
                    turned.set(Some(now_ms()));
                });
            }
            spawn_local(async move {
                if h.state.storage().sync().await.is_err() || h.dying.get() {
                    return;
                }
                let synced_at = now_ms();
                let sync = synced_at.saturating_sub(began) as u32;
                let turn = turned.get().map_or(sync, |t: u64| (t.saturating_sub(began) as u32).min(sync));
                let applied = h.core.try_borrow().ok().and_then(|c| c.as_ref().map(|c| c.applied.0)).unwrap_or(0);
                h.trace(Trace::Confirm, 0, 0, applied, now_ms().saturating_sub(began));
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
                        if s.confirm_split.len() < SAMPLES_MAX {
                            let gate = began.saturating_sub(t0) as u32;
                            s.confirm_split.push(ConfirmSplit { gate, sync, turn, stmts: n_stmts, bytes: n_bytes });
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
        let deps_used = out.deps_used;
        let catchup = out.catchup;
        let installs = out.install;
        let mut lost = None;
        {
            let Ok(mut guard) = self.core.try_borrow_mut() else { return back };
            let Some(core) = guard.as_mut() else { return back };
            let now = now_ms();
            // The answers this input built, by query: valid only at this input's grants (`shared_answer`).
            let mut built: HashMap<String, ReadBody> = HashMap::new();
            let every = crate::family::dump_every(core.m.me());
            // A refusal resolves only once its entry commits, in this output or a later one: its ticket keeps the
            // command until then, for the lease loop.
            for (_, rid, cmd) in out.refused {
                if let Some(a) = core.admitted.values_mut().find(|(r, _, _)| *r == rid) {
                    a.2 = Some(cmd);
                }
            }
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
                        let blocked = matches!(&result, Err(CommitError::Refused { err: e, .. }) if F::needs_block(e));
                        if blocked && let Some((rid, _, Some(cmd))) = admitted {
                            self.blocked.borrow_mut().push((ticket, rid, cmd));
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
                        if let (Ok(a), Some((rid, _, Some(cmd)))) = (&result, admitted) {
                            #[cfg(feature = "test-faults")]
                            let skip = core.no_marker.remove(&rid);
                            #[cfg(not(feature = "test-faults"))]
                            let skip = {
                                let _ = rid;
                                false
                            };
                            if !skip {
                                marks.push((cmd, a.clone()));
                            }
                        }
                        for tx in core.waiters.remove(&ticket).unwrap_or_default() {
                            let _ = tx.send(result.clone());
                        }
                    }
                    Resolution::Read { id, result } => {
                        // A read whose caller has gone builds nothing: in B0-35 a leader's one alarm input granted
                        // about 730 reads whose callers had timed out and dumped both tiers for each, 641 s of CPU
                        // in which the isolate answered nothing, and the group lost its leader for 10 minutes
                        // (`tests/am1/results/lm1-memory.md`, "The leaderless group").
                        if let Some((q, tx)) = core.reads.remove(&id).filter(|(_, tx)| !tx.is_canceled()) {
                            // `fresh` only before the member's next input: this runs right after the input. A `dep`
                            // read answers the whole fold in the crate's codec, hex, for another group's `deps`.
                            let v = result.and_then(|g| {
                                core.m.fresh(&g).map(|f| {
                                    let floor = g.floor().0;
                                    let (body, hit) = shared_answer(&mut built, &q, floor, || {
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
                                            return self.timed_answer(f.fold(), &q, floor, now, every);
                                        };
                                        ReadBody::Json(
                                            serde_json::to_vec(&body).unwrap_or_else(|_| b"null".to_vec()).into(),
                                        )
                                    });
                                    if hit {
                                        self.stats.borrow_mut().shared_reads += 1;
                                    }
                                    // A read whose answer is not built yet keeps its own grant, for `answer_after`.
                                    let body = match body {
                                        ReadBody::Shed(ms, _) => ReadBody::Shed(ms, Some(g.clone())),
                                        b => b,
                                    };
                                    (body, f.index())
                                })
                            });
                            // An answer read while storage refused a row is not sent: the caller sees a reset.
                            if let Some(why) = self.lost_cause(&core.m) {
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
                    // why-logs: the window the trip judged, its own samples, so a candidate rule is replayed on them.
                    if let (graftyard_raft::Alarm::StepDown { .. }, Some(w)) = (&a, core.m.step_down_window()) {
                        console_log!("{}", step_down_line(w));
                    }
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
        #[cfg(feature = "test-faults")]
        for used in deps_used {
            let reads: Vec<Value> = match self.core.try_borrow_mut() {
                Ok(mut c) => c
                    .as_mut()
                    .map(|c| used.ids.iter().flat_map(|id| c.dep_reads.remove(id).unwrap_or_default()).collect())
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let _ = self.state.storage().sql().exec(
                "INSERT INTO deps_log (idx, at, reads) VALUES (?, ?, ?)",
                vec![i(used.index.0), i(now_ms()), json!(reads).to_string().into()],
            );
        }
        #[cfg(not(feature = "test-faults"))]
        let _ = deps_used;
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
        let held = match &e.msg {
            raft::Message::Append(a) => {
                Some((e.to.0, a.round, a.entries.last().map_or(a.prev_index.0, |x| x.index.0), a.commit.0))
            }
            _ => None,
        };
        // Held bytes are counted by the entries' encoding, as the log counts them (`raft::Member::log_bytes`).
        let bytes = match &e.msg {
            raft::Message::Append(a) => a.entries.iter().map(|x| raft::encode_entry(x).len()).sum(),
            _ => 0,
        };
        let Some(e) = self.callers.borrow_mut().route(e.to, e, bytes, now_ms()) else {
            if let Some((to, round, last, commit)) = held {
                self.trace(Trace::AppendHeld, to, round, last, commit);
            }
            #[cfg(feature = "test-faults")]
            self.mark_mem();
            return;
        };
        self.call(e);
    }

    /// Sends `e` as a member-to-member call and steps the replies.
    fn call(self: &Rc<Self>, e: Envelope<F>) {
        let h = self.clone();
        let queued = now_ms();
        spawn_local(async move {
            let body = codec::to_bytes(std::slice::from_ref(&e));
            let _counted = Counted::new(&h.call_bytes, body.len());
            #[cfg(feature = "test-faults")]
            h.mark_mem();
            let call = Call::of(&e, body.len());
            let traced = call.append.filter(|_| call.entries > 0);
            if let Some((round, _, last)) = traced {
                h.trace_at(queued, Trace::AppendQueued, call.to, round, last, body.len() as u64);
            }
            match (&e.msg, call.batch) {
                (raft::Message::ReadIndex { .. }, Some(b)) => h.trace(Trace::IndexSent, e.to.0, b, 0, 0),
                (raft::Message::ReadIndexRes { index, .. }, Some(b)) => {
                    h.trace(Trace::ResSent, e.to.0, b, index.map_or(0, |(i, _)| i.0), 1)
                }
                _ => {}
            }
            // What `call_failed` needs, so the envelope and its entries are not held while a late answer is awaited.
            let (to, on_miss, on_error) = (e.to, e.failure(true), e.failure(false));
            // However the call ends, its AppendEntries bytes stop counting in the core's budget (`call_ended`).
            let _ended = Ended(
                h.clone(),
                match &e.msg {
                    raft::Message::Append(a) if a.boot == h.boot() => Some((to, a.round)),
                    _ => None,
                },
            );
            #[cfg(feature = "test-faults")]
            let drop_reply = matches!(e.msg, raft::Message::Append(_));
            drop(e);
            #[cfg(feature = "test-faults")]
            if let Some(hang) = crate::faults::site_fault(&h.env, h.chain(), true).await {
                crate::faults::count(&format!("site_fault:out>{}", to.0));
                // A hang past CALL_DEADLINE is a missed deadline, as a blackholed call is (B0-07's gray site).
                if hang >= raft::CALL_DEADLINE.as_millis() as u64 {
                    Delay::from(raft::CALL_DEADLINE).await;
                    h.ended(&call, End::Failed(PeerErr::new(CallFail::Deadline, "site fault: hung")));
                    h.stats.borrow_mut().failed_calls += 1;
                    return h.call_failed(on_miss);
                }
                Delay::from(Duration::from_millis(hang)).await;
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "site fault")));
                return h.call_failed(on_error);
            }
            #[cfg(feature = "test-faults")]
            if h.severed() {
                crate::faults::count(&format!("severed:out>{}", to.0));
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "severed")));
                return h.call_failed(on_error);
            }
            #[cfg(feature = "test-faults")]
            if crate::faults::fail_call(&h.env, to.0) {
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Injected, "fail_call")));
                return h.call_failed(on_error);
            }
            #[cfg(feature = "test-faults")]
            call.hop.set(h.hop.get() + 1);
            let answered = Rc::new(Cell::new(false));
            if h.is_learner() {
                h.hedge(to, body.clone(), call.hop.get(), answered.clone());
            }
            let missed = || {
                h.ended(&call, End::Failed(PeerErr::new(CallFail::Deadline, "no answer within CALL_DEADLINE")));
                h.stats.borrow_mut().failed_calls += 1;
                h.call_failed(on_miss);
            };
            // `answer` starts the limit's timer only once the deadline passed.
            let issued = now_ms();
            if let Some((round, _, last)) = traced {
                h.trace(Trace::AppendIssued, call.to, round, last, issued.saturating_sub(queued));
            }
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
            if let Some((round, _, _)) = traced {
                h.trace(Trace::AppendEnded, call.to, round, u64::from(r.is_err()), now_ms().saturating_sub(issued));
            }
            answered.set(true);
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
            if drop_reply && (now_ms() < h.half_hung_until.get() || crate::faults::drop_append_reply(&h.env, to.0)) {
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
                h.heard(&r);
                h.trace_res(&r, true);
                if h.input(|m, t| m.step(t, from, r.msg)).is_err() {
                    break;
                }
            }
        });
    }

    /// This boot's id, which its AppendEntries carry.
    fn boot(&self) -> u64 {
        self.core.try_borrow().ok().and_then(|c| c.as_ref().map(|c| c.m.boot())).unwrap_or(0)
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
        // B0-07's `SiteHealth` stand-in: each call to a voter's site, once, at its first outcome.
        #[cfg(feature = "test-faults")]
        if c.to < graftyard_types::LEARNER_ADDR && matches!(end, End::Ok | End::Failed(_)) {
            crate::faults::site_health(&self.env, c.to, matches!(end, End::Ok));
        }
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
        p.sent += 1;
        p.sent_bytes += c.bytes as u64;
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
        // The body leaves through a fixed-length stream, not as one array: a SQLite write that starts while an
        // array body is still in flight stalled that body 100 to 1,000 ms in about 1% of calls, and a streamed body
        // did not stall more than a call with no write at all (`tests/m0/results/carried-4.md`, B0-02's leader stall).
        let mut init = RequestInit::new();
        let chunk = js_sys::Uint8Array::from(body.as_slice());
        drop(body);
        #[cfg(feature = "test-faults")]
        let array = self.array_body.get();
        #[cfg(not(feature = "test-faults"))]
        let array = false;
        if array {
            init.with_method(Method::Post).with_body(Some(chunk.into()));
        } else {
            let stream = worker::worker_sys::FixedLengthStream::new(chunk.length())
                .map_err(|x| PeerErr::new(CallFail::Request, format!("{x:?}")))?;
            let writer =
                stream.writable().get_writer().map_err(|x| PeerErr::new(CallFail::Request, format!("{x:?}")))?;
            init.with_method(Method::Post).with_body(Some(stream.readable().into()));
            // A write or close refused (the call failed and its body was dropped) surfaces as the fetch's own error.
            let (wrote, closed) = (writer.write_with_chunk(&chunk), writer.close());
            spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(wrote).await;
                let _ = wasm_bindgen_futures::JsFuture::from(closed).await;
            });
        }
        let h = Headers::new();
        let _ = h.set(crate::clock::STAMP, &self.now().to_string());
        #[cfg(feature = "test-faults")]
        let (id, awaiting) = {
            let id = self.call_seq.get() + 1;
            self.call_seq.set(id);
            // Counted until the response headers arrive, or until the call is dropped unanswered.
            let awaiting = Counted::new(&self.awaiting, 1);
            let sent = now_ms();
            let _ = h.set("x-graft-hop", &hop.to_string());
            let _ = h.set("x-graft-sent", &sent.to_string());
            let _ = h.set("x-graft-call", &id.to_string());
            // The sender's address, so the callee traces `CallArrived` without decoding the body a second time.
            if let Some(me) = self.core.borrow().as_ref().map(|c| c.m.me()) {
                let _ = h.set("x-graft-from", &me.0.to_string());
            }
            // No I/O runs between this stamp and the fetch below, so the fetch is issued at this clock value.
            self.trace_at(sent, Trace::CallSent, to.0, id, self.awaiting.get() as u64, self.loop_lag());
            (id, awaiting)
        };
        init.with_headers(h);
        #[cfg(not(feature = "test-faults"))]
        let _ = hop;
        let req =
            Request::new_with_init("https://member/raft", &init).map_err(|x| PeerErr::new(CallFail::Request, x))?;
        let mut res = stub.fetch_with_request(req).await.map_err(|x| PeerErr::new(CallFail::Fetch, x))?;
        self.stamped(res.headers().get(crate::clock::STAMP).ok().flatten());
        #[cfg(feature = "test-faults")]
        drop(awaiting);
        #[cfg(feature = "test-faults")]
        if let Some(left) = res.headers().get("x-graft-left").ok().flatten().and_then(|v| v.parse().ok()) {
            self.trace(Trace::AnswerArrived, to.0, id, left, 0);
        }
        let status = res.status_code();
        if status != 200 {
            let text = res.text().await.unwrap_or_default();
            return Err(PeerErr::new(CallFail::Status, format!("{status} {text}")));
        }
        let b = res.bytes().await.map_err(|x| PeerErr::new(CallFail::Body, x))?;
        codec::from_bytes::<Vec<Envelope<F>>>(&b).map_err(|x| PeerErr::new(CallFail::Decode, x))
    }

    /// Whether this member is a learner.
    fn is_learner(&self) -> bool {
        self.core.borrow().as_ref().is_some_and(|c| graftyard_types::learner_slot(c.m.me()).is_some())
    }

    /// A learner's call with no answer within `HEDGE` is sent once more, and the copy's replies are stepped as they
    /// arrive. A learner keeps one read-index batch in flight, so one call delayed on the network holds every read
    /// queued behind it until the call ends or CALL_DEADLINE resends it: B0-18's last failures on `graft-m2b` were
    /// single calls between apac and enam that took 1.1 to 2 s (`tests/m3/results/m3-wp5.md`). Raft takes the copy as
    /// it takes any duplicate: a second answer to one batch or round is ignored. Both responses are stepped, so what
    /// a leader held for this learner reaches it in either one.
    fn hedge(self: &Rc<Self>, to: SiteId, body: Vec<u8>, hop: u64, answered: Rc<Cell<bool>>) {
        let h = self.clone();
        spawn_local(async move {
            Delay::from(HEDGE).await;
            if answered.get() || h.dying.get() {
                return;
            }
            let at = now_ms();
            let r = h.call_peer(to, body, hop).await;
            h.trace(Trace::Hedge, to.0, 0, u64::from(r.is_ok()), now_ms().saturating_sub(at));
            for e in r.unwrap_or_default() {
                let from = e.from;
                h.heard(&e);
                h.trace_res(&e, true);
                if h.input(|m, t| m.step(t, from, e.msg)).is_err() {
                    break;
                }
            }
        });
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

    /// `fold_answer`, with the ms of each narrow answer it builds kept in `Stats::narrow_build_ms`.
    fn timed_answer(&self, fold: &F::Fold, q: &Value, floor: u64, now: u64, every: u64) -> ReadBody {
        let (n, t) = (crate::family::narrow_builds(), now_ms());
        let body = fold_answer::<F>(fold, q, floor, now, every);
        if crate::family::narrow_builds() > n {
            let mut s = self.stats.borrow_mut();
            if s.narrow_build_ms.len() < SAMPLES_MAX {
                s.narrow_build_ms.push(now_ms().saturating_sub(t) as u32);
            }
        }
        body
    }

    /// The answer to read `q`, granted in an earlier input as `g`, at this member's state now
    /// (`raft::Member::fresh_after`): for a read whose answer was not built at its grant (`ReadBody::Shed`), once the
    /// build is due, with no second read-index round. It may be `Shed` again when another read took the build slot
    /// with an answer below this read's floor. `None` when the object is resetting.
    pub fn answer_after(self: &Rc<Self>, g: &raft::ReadGrant, q: &Value) -> Option<ReadResult> {
        let now = now_ms();
        let (r, lost) = {
            let core = self.core.borrow();
            let c = core.as_ref()?;
            let every = crate::family::dump_every(c.m.me());
            let r = c.m.fresh_after(g).map(|f| {
                let body = match self.timed_answer(f.fold(), q, g.floor().0, now, every) {
                    ReadBody::Shed(ms, _) => ReadBody::Shed(ms, Some(g.clone())),
                    b => b,
                };
                (body, f.index())
            });
            (r, self.lost_cause(&c.m))
        };
        if let Some(why) = lost {
            self.fail_lost(&why);
            return None;
        }
        Some(r)
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
        let emitted = now_ms();
        spawn_local(async move {
            // `depsMs.spawn`: from the release that emitted the `DepsWanted` to this task's start, the isolate's own
            // queueing ahead of the reads (round 6: dep reads took 118 ms p50 under load against 28 ms idle).
            {
                let mut s = h.stats.borrow_mut();
                let v = s.deps_ms.entry("spawn".to_owned()).or_default();
                if v.len() < SAMPLES_MAX {
                    v.push(now_ms().saturating_sub(emitted) as u32);
                }
            }
            #[cfg(feature = "test-faults")]
            {
                let hold = h.core.borrow().as_ref().map_or(0, |c| c.hold_deps_until);
                if hold > now_ms() {
                    Delay::from(Duration::from_millis(hold - now_ms())).await;
                }
            }
            // Each chain at once, as "The write path" reads them: one after another cost the sum of three reads,
            // about 1.3 s of every merge (`tests/m2/results/m2-wp6.md`, "Merge latency").
            let began = now_ms();
            let joined = h.core.borrow().as_ref().map(|c| c.join.clone());
            let one = |chain: graftyard_types::ChainId| {
                let (env, hedges) = (&h.env, &h.stats);
                let known = joined.as_ref().and_then(|j| owner_chain_record(j, chain));
                async move {
                    let t0 = now_ms();
                    let q = &json!({ "q": { "dep": true } });
                    // `dep-hedge`: a read with no answer within its chain's delay (its p90 here, up to DEP_HEDGE) sends a second.
                    let read = |record: graftyard_types::PlacementRecord| async move {
                        let (r, how) = crate::lookup::call_hedged(env, chain, &record, "/read", q).await;
                        if how != graftyard_lookup::Hedged::First {
                            let mut s = hedges.borrow_mut();
                            s.dep_hedges[0] += 1;
                            s.dep_hedges[1] += u32::from(how == graftyard_lookup::Hedged::Won);
                        }
                        r
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
            {
                let mut s = h.stats.borrow_mut();
                s.dep_reads += 1;
                let all = ("all".to_owned(), Some(now_ms().saturating_sub(began)));
                let each = reads.iter().map(|r| {
                    let kind = r["chain"].as_str().and_then(|c| c.split(':').next()).unwrap_or("?").to_owned();
                    (kind, r["ms"].as_u64())
                });
                for (kind, ms) in std::iter::once(all).chain(each) {
                    match ms {
                        Some(ms) => {
                            let v = s.deps_ms.entry(kind).or_default();
                            if v.len() < SAMPLES_MAX {
                                v.push(ms as u32);
                            }
                        }
                        None => *s.dep_fails.entry(kind).or_default() += 1,
                    }
                }
            }
            // The entry these reads decide is logged once `DepsUsed` names it: with reads overlapping the entry in
            // flight, an id block or record may be appended first, and a re-read may join a read for one entry.
            #[cfg(feature = "test-faults")]
            if let Ok(mut c) = h.core.try_borrow_mut()
                && let Some(c) = c.as_mut()
            {
                c.dep_reads.insert(want.id, reads.clone());
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
                let marked = F::marked(&cmd);
                let (r2, out) = match h.step(|m, t| m.commit(t, rid, cmd)) {
                    Ok(x) => x,
                    Err(_) => break,
                };
                match r2 {
                    Ok(a) => {
                        if let Some(core) = h.core.borrow_mut().as_mut() {
                            let at = now_ms();
                            core.admitted.insert(a.ticket, (rid, at, marked));
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
            other => Err(format!("block {block:?} did not commit: {other:?}")),
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
        #[cfg(feature = "test-faults")]
        let tick_lag = self.tick_lag.clone();
        spawn_local(async move {
            #[cfg(feature = "test-faults")]
            let mut late = 0;
            loop {
                let due = now_ms() + TICK.as_millis() as u64;
                #[cfg(feature = "test-faults")]
                tick_lag.set((late, due));
                Delay::from(TICK).await;
                let Some(h) = w.upgrade() else { return };
                let lag = now_ms().saturating_sub(due);
                #[cfg(feature = "test-faults")]
                {
                    late = lag;
                }
                if lag >= 20 {
                    h.trace(Trace::Lag, 0, 0, 0, lag);
                }
                if h.dying.get() || (now_ms() > h.active_until.get() && !h.has_waiters()) || h.tick_once().is_err() {
                    h.ticking.set(false);
                    #[cfg(feature = "test-faults")]
                    h.tick_lag.set((0, 0));
                    return;
                }
            }
        });
    }

    /// B0-07: the site modes' stand-in, read on every tick (every TICK while active, and from the leader's drain-check
    /// alarm), stepped into the core when the sites that may not lead this group change.
    #[cfg(feature = "test-faults")]
    fn apply_site_modes(self: &Rc<Self>) -> Result<()> {
        // Keyed by the core's boot, so a core rebuilt in this object gets the set again.
        let Some((chain, boot)) =
            self.core.borrow().as_ref().map(|c| (c.join.chain.to_string(), c.m.boot().to_string()))
        else {
            return Ok(());
        };
        let set = crate::faults::site_modes(&self.env).may_not_lead(&chain);
        let key = (boot, set.clone());
        if self.drained.borrow().as_ref() == Some(&key) {
            return Ok(());
        }
        *self.drained.borrow_mut() = Some(key);
        let until = self.active_until.get();
        let r = self.step(|m, t| m.set_drained(t, set.into_iter().map(SiteId)));
        self.active_until.set(until);
        let ((), out) = r?;
        self.dispatch(out, None);
        Ok(())
    }

    /// One `tick`, which does not count as activity, so the ticker winds down when inputs stop.
    fn tick_once(self: &Rc<Self>) -> Result<()> {
        #[cfg(feature = "test-faults")]
        self.apply_site_modes()?;
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

    /// Arms the object's one alarm `ms` from now, unless one is set sooner and not yet overdue.
    pub fn arm_in(self: &Rc<Self>, ms: i64) {
        let at = now_ms() + ms as u64;
        if !rearm(self.alarm_at.get(), at, now_ms()) {
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
        if !leading {
            self.keep_led();
        }
        // The alarm this invocation armed is set again, awaited, before it returns. One armed only from the task
        // `arm_in` spawns can be lost as the invocation ends [inference]: in BF-57 run 10 (graft-m4a) a leader's alarm
        // armed `arm` 2 s out at 90 s, none ran after it, and five committed entries waited 137 s for an unrelated
        // read to wake the member (`tests/m4/results/m4-wp1-hosted.md`).
        let at = self.alarm_at.get();
        let opts = SetAlarmOptions { allow_unconfirmed: Some(true), ..Default::default() };
        if at != 0
            && let Err(e) = self.state.storage().set_alarm_with_options(at.saturating_sub(now_ms()) as i64, opts).await
        {
            let mut s = self.stats.borrow_mut();
            if s.alarms.len() < 50 {
                s.alarms.push(format!("{} set alarm at end: {e}", now_ms()));
            }
        }
    }

    /// A voter whose group has leader-only background work (`Family::needs_leader`) checks every KEEP_LED_EVERY,
    /// and once it has seen no leader for 2 x ELECTION_AFTER probes the leader it names or campaigns (`keep_led`).
    fn keep_led(self: &Rc<Self>) {
        let (heard, named, lapsed) = {
            let core = self.core.borrow();
            let Some(c) = core.as_ref() else { return };
            if graftyard_types::learner_slot(c.m.me()).is_some() {
                return;
            }
            let named = c.m.leader().is_some_and(|l| l != c.m.me());
            let until = c.m.state().promise_until.0;
            (c.m.role() == Role::Leader || (named && now_ms() < until), named, until)
        };
        if !F::needs_leader(self) || !F::may_lead(self) {
            return self.unheard.set((0, 0));
        }
        // The core's timers: an unanswered probe is failed, and its leader forgotten, only on a tick.
        let _ = self.tick_once();
        let now = now_ms();
        let (found, acted) = match self.unheard.get() {
            _ if heard => (0, 0),
            (0, a) => (now, a),
            u => u,
        };
        let mut next = KEEP_LED_EVERY;
        match keep_led(heard, named, true, lapsed, found, acted, now) {
            KeepLed::Off | KeepLed::Wait => self.unheard.set((found, acted)),
            KeepLed::Probe => {
                self.unheard.set((found, now));
                let r = self.input(|m, t| m.read_index(t));
                console_log!("keep led: no leader seen for 2 x ELECTION_AFTER with work due: probe {r:?}");
                next = PROBED_CHECK;
            }
            KeepLed::Campaign => {
                self.unheard.set((found, now));
                let v = self.input(|m, t| m.campaign(t));
                console_log!("keep led: no leader seen for 2 x ELECTION_AFTER with work due: campaign {v:?}");
                self.stats.borrow_mut().keep_led += 1;
            }
        }
        self.arm_in(next);
    }

    /// Counts a failed write that SQLite refused for space (B0-30's `SQLITE_FULL`), and logs it, so a reader of
    /// Workers Logs finds it after the reset this write causes.
    pub fn full(&self, e: &str) {
        if out_of_space(e) {
            self.stats.borrow_mut().sqlite_full += 1;
            console_error!("SQLITE_FULL: {e}");
        }
    }

    /// A storage write failed: reset the object. Every input from now on is refused, so nothing else leaves.
    fn fail(self: &Rc<Self>) {
        self.dying.set(true);
        abort(&self.state, "graftyard member storage write failed");
    }

    /// Whether a fold read storage refused (`Host::lost_cause`); if so the object is resetting, and the caller must let
    /// nothing it computed from the fold leave: no reply, no batch, no snapshot part.
    pub fn fold_lost(self: &Rc<Self>) -> bool {
        let why = self.core.try_borrow().ok().and_then(|c| c.as_ref().and_then(|c| self.lost_cause(&c.m)));
        match why {
            Some(why) => {
                self.fail_lost(&why);
                true
            }
            None => false,
        }
    }

    /// Why a read of `m`'s fold storage failed: a fold tier (`Family::lost`) or the `request` table
    /// (`RequestSql::lost`).
    fn lost_cause(&self, m: &raft::Member<F::C>) -> Option<String> {
        F::lost(&m.group_fold().fold).or_else(|| self.requests.borrow().as_ref().and_then(RequestSql::lost))
    }

    /// A fold read storage refused (`Family::lost`): reset this object, as a failed write does. Only this member
    /// resets; a panic would rebuild the wasm instance and reset every member in the isolate (F47's site a panic).
    fn fail_lost(self: &Rc<Self>, why: &str) {
        console_error!("member reset: fold storage lost: {why}");
        // A dump read since the loss may lack rows: none is served again from the isolate's cache.
        crate::family::forget_dumps();
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

    /// Committed entries `from..=to` from storage, at most 2,000 and about `page_bytes` of stored rows (always one), each with its index and the family's fields
    /// (`Family::entry_json`): what the git cache shard's `ChainLog::entries` reads.
    pub fn entries_json(&self, from: u64, to: u64, page_bytes: usize) -> Result<Value> {
        let commit = self.core.borrow().as_ref().map_or(0, |c| c.m.state().commit.0);
        let to = to.min(commit);
        let sql = self.state.storage().sql();
        let mut out = Vec::new();
        let mut bytes = 0;
        // Rows are read one at a time and the read stops at the page's end: collecting all 2,000 first held up to
        // 2,000 rows (about 47 MB of repo:65536's 23.5 KB entries) per call and cost its leader about 160 ms of CPU a
        // page through a rehydrate's 420 pages (`tests/m3/results/wire-fix.md`).
        // Read in pages read to their end (`crate::logread::log_after`), so stopping at the page's end leaves no
        // statement open.
        for r in crate::logread::log_after(&sql, "row", from.max(1) - 1) {
            let r = r?;
            // A page stops past `page_bytes` (and always holds one entry), so a log of inline packs answers well
            // within the lookup's CALL; the shard asks again from the next index.
            let row = blob(&r[1])?;
            bytes += row.len();
            if int(&r[0])? as u64 > to || out.len() == 2000 || (!out.is_empty() && bytes > page_bytes) {
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

    /// The member's stored log, with each entry's request ids, from `from`: at most `n` entries and about
    /// `LOG_JSON_BYTES` of JSON (`log_take`). The rows are read a page at a time (`crate::logread::log_after`), so the
    /// read holds one page and the answer, not `n` whole entries: on b030r6 a harness read of 100 to 400 entries, each
    /// with its ~320 KB inline pack, took the leader's isolate past its memory limit (2026-11-17 17:36Z and 18:38Z).
    #[cfg(feature = "test-faults")]
    pub fn log_json(&self, from: u64, n: usize) -> Result<Value> {
        let core = self.core.borrow();
        let Some(core) = core.as_ref() else { return Ok(json!([])) };
        let commit = core.m.state().commit.0;
        let sql = self.state.storage().sql();
        let rows = crate::logread::log_after(&sql, "digest, row", from.max(1) - 1).map(|r| -> Result<(Value, usize)> {
            let r = r?;
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
            let len = serde_json::to_string(&row).map_or(usize::MAX, |s| s.len());
            Ok((row, len))
        });
        Ok(json!(log_take(rows, n, LOG_JSON_BYTES)?))
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
/// `GRAFTYARD_REQUEST_RETENTION_S` variable, which every site of an install sets to the same value. On a tiered
/// (repository) chain `GRAFTYARD_REPO_REQUEST_RETENTION_S` wins: B0-30 replays 30 days in 40 h and scales the 48 h
/// by the same factor, which a non-tiered chain's restore check (`load`) would refuse on groups that kept 48 h.
pub fn request_retention(env: &Env, tiered: bool) -> Duration {
    #[cfg(feature = "test-faults")]
    for (var, on) in [("GRAFTYARD_REPO_REQUEST_RETENTION_S", tiered), ("GRAFTYARD_REQUEST_RETENTION_S", true)] {
        if on && let Some(s) = env.var(var).ok().and_then(|v| v.to_string().parse().ok()) {
            return Duration::from_secs(s);
        }
    }
    let _ = (env, tiered);
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

/// How long past its time an alarm `arm_in` set may be before it counts as lost [guess]. At 5 s, a leader whose alarm
/// invocation was canceled still stalled its alarm-driven consumers 5.5 to 6.3 s three times in BF-57 run 6
/// (graft-m4a), each from its last alarm to the first input after the 5 s; an alarm that runs reaches `alarm` well
/// within a second of its time, and re-arming a live one only moves it.
const ALARM_LATE_MS: u64 = 1_000;

/// Whether `arm_in` sets the alarm for `at`, with `armed` the one it set last (0 for none) at `now`: when there is
/// none, when `at` is sooner, or when the armed one is `ALARM_LATE_MS` overdue. A canceled alarm invocation never
/// runs `alarm`, which clears `armed`; before the last case, every `arm_in` after one saw the stale time as sooner and
/// set nothing, and a leader's alarm-driven consumers stopped for 50 s, until an unrelated input re-armed it
/// (graft-m4a, BF-57 run 5, `tests/m4/results/m4-wp1-hosted.md`).
fn rearm(armed: u64, at: u64, now: u64) -> bool {
    armed == 0 || at < armed || now > armed + ALARM_LATE_MS
}

/// A request id in its text form, the ULID's 26 characters.
pub fn rid_from(s: &str) -> Result<RequestId> {
    s.parse().map_err(|_| err(format!("bad request id {s:?}")))
}

/// The JSON `GET /log` answers at most (`Host::log_json`), about.
#[cfg(any(test, feature = "test-faults"))]
const LOG_JSON_BYTES: usize = 4 << 20;

/// `GET /log`'s rows, each with its JSON length, taken while they fit: at most `n`, and the walk stops at the first
/// row that would take the answer past `cap` bytes, which is left out, so a caller asking again from the last index
/// plus one gets it next. The first row is always taken, so one entry over `cap` still reads. Stops on an error.
#[cfg(any(test, feature = "test-faults"))]
fn log_take(rows: impl Iterator<Item = Result<(Value, usize)>>, n: usize, cap: usize) -> Result<Vec<Value>> {
    let (mut out, mut bytes) = (Vec::new(), 0usize);
    for r in rows.take(n) {
        let (row, len) = r?;
        if !out.is_empty() && bytes.saturating_add(len) > cap {
            break;
        }
        bytes = bytes.saturating_add(len);
        out.push(row);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    /// A trip's window logs its bounds and every sample, and `slow` counts only confirms over `confirm_p99_ms`: one of
    /// exactly 500 ms is not slow, as `STEP_DOWN`'s rule reads it.
    #[test]
    fn a_step_down_window_logs_its_samples_and_slow_count() {
        use graftyard_raft::StepDownWindow;
        use graftyard_types::Timestamp;
        let w = StepDownWindow {
            from: Timestamp(1_000),
            to: Timestamp(11_000),
            confirms: vec![(Timestamp(2_000), 500), (Timestamp(3_000), 501), (Timestamp(4_000), 40)],
            kept: vec![(Timestamp(500), 900)],
        };
        let v: serde_json::Value = serde_json::from_str(&super::step_down_line(&w)).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "stepDownWindow": { "from": 1000, "to": 11000, "slow": 1, "of": 3,
                "confirms": [[2000, 500], [3000, 501], [4000, 40]], "kept": [[500, 900]] } })
        );
    }

    /// `GET /log` stops before the row that would cross its byte cap, still takes a first row over it, and holds `n`:
    /// a harness read of 400 entries with ~320 KB packs answered ~128 MB and reset b030r6's leader.
    #[test]
    fn the_log_read_stops_at_its_byte_cap() {
        use serde_json::json;
        let read = std::cell::Cell::new(0usize);
        let big = |k: usize| {
            let read = &read;
            (1..=k).map(move |i| {
                read.set(read.get() + 1);
                Ok((json!({ "index": i }), 330_000))
            })
        };
        let cap = super::LOG_JSON_BYTES;
        let got = super::log_take(big(400), 400, cap).unwrap();
        // 12 rows of 330,000 B are 3,960,000 B; the 13th would make 4,290,000 > 4,194,304.
        assert_eq!(got.len(), 12);
        assert_eq!(got.last().unwrap()["index"], 12);
        assert_eq!(read.get(), 13, "the walk stops at the row that crosses the cap, not at n");
        // Under the cap, `n` holds; a row exactly at the cap is taken.
        assert_eq!(super::log_take(big(400), 5, cap).unwrap().len(), 5);
        let exact = vec![Ok((json!(1), cap - 10)), Ok((json!(2), 10)), Ok((json!(3), 1))];
        assert_eq!(super::log_take(exact.into_iter(), 10, cap).unwrap().len(), 2);
        // One entry over the cap still reads, alone.
        let huge = vec![Ok((json!(1), cap + 1)), Ok((json!(2), 1))];
        assert_eq!(super::log_take(huge.into_iter(), 10, cap).unwrap().len(), 1);
    }

    /// A voter with leader-only work acts once it has seen no leader for 2 x ELECTION_AFTER: ELECTION_AFTER after its
    /// promise lapsed, even when its first check came later, or after it was first found so if it never promised; then
    /// at most every ELECTION_AFTER. It probes a leader it still names and campaigns when it names none. One that leads
    /// or still holds its promise, or has no such work, does nothing.
    /// The platform's database size limit counts as out of space, as SQLite's own refusals do: b030r4's site c hit it
    /// at 10.70 GB while its stats read `sqliteFull: 0`. Other storage errors do not count.
    #[test]
    fn the_platform_s_size_limit_counts_as_sqlite_full() {
        assert!(super::out_of_space("Error: Exceeded the maximum database size."));
        assert!(super::out_of_space("SQLITE_FULL: database or disk is full"));
        assert!(!super::out_of_space("Error: database table is locked: SQLITE_LOCKED"));
        assert!(!super::out_of_space(
            "Durable Object storage operation exceeded timeout which caused object to be reset."
        ));
    }

    #[test]
    fn a_leaderless_voter_with_work_due_acts_after_the_bound_only() {
        use super::KeepLed::*;
        use super::keep_led as k;
        let e = graftyard_raft::ELECTION_AFTER.as_millis() as u64;
        assert_eq!(super::LEADERLESS_FOR.as_millis() as u64, e);
        let lapsed = 1_000_000;
        // Its first check came 5 s after the lapse: the bound still runs from the lapse.
        let found = lapsed + 5_000;
        assert_eq!(k(false, false, true, lapsed, found, 0, lapsed + e - 1), Wait);
        assert_eq!(k(false, false, true, lapsed, found, 0, lapsed + e), Campaign);
        assert_eq!(k(false, true, true, lapsed, found, 0, lapsed + e), Probe, "it still names a leader");
        // Never promised: from when it was found.
        assert_eq!(k(false, false, true, 0, found, 0, found + e - 1), Wait);
        assert_eq!(k(false, false, true, 0, found, 0, found + e), Campaign);
        // After acting, the next waits ELECTION_AFTER from then.
        let acted = lapsed + e;
        assert_eq!(k(false, false, true, lapsed, found, acted, acted + e - 1), Wait);
        assert_eq!(k(false, false, true, lapsed, found, acted, acted + e), Campaign);
        assert_eq!(k(true, true, true, lapsed, found, 0, lapsed + 10 * e), Off, "its promise runs");
        assert_eq!(k(false, false, false, lapsed, found, 0, lapsed + 10 * e), Off, "no leader-only work");
    }

    /// A paced write waits twice its own entry's commit, nothing after an instant one, and never over PACE_MAX.
    #[test]
    fn paced_writes_wait_twice_their_commit_up_to_the_cap() {
        use std::time::Duration;
        assert_eq!(super::paced(450), Duration::from_millis(900));
        assert_eq!(super::paced(0), Duration::ZERO);
        assert_eq!(super::paced(60_000), super::PACE_MAX);
    }

    /// The election rows split a leaderless span: a pre-vote's requests leave as candidate-to-be (no role change), the
    /// vote's step moves Follower to Candidate, the win moves Candidate to Leader, and only the leader's first commit
    /// move in its term is a `FirstCommit` row; a later commit, or a follower's, is not.
    #[test]
    fn election_rows_mark_pre_vote_vote_win_and_one_first_commit_per_term() {
        use super::{Elect, elect_rows};
        use graftyard_raft::{Envelope, Index, Message, Out, Resolution, Role, Term, Verdict, VoteReq};
        use graftyard_types::{SiteId, Timestamp};
        type C = graftyard_git::repo::RepoCmd;
        let vote = |to: u8, term: u64, pre: bool| Envelope {
            from: SiteId(1),
            to: SiteId(to),
            msg: Message::Vote(VoteReq {
                term: Term(term),
                candidate: SiteId(1),
                pre,
                last_index: Index(40),
                last_term: Term(3),
                transfer: None,
                boots: [Timestamp(0); 2],
                stepped_down_at: None,
                bootstrapped: true,
            }),
        };
        let kinds = |rows: &[[u64; 6]]| rows.iter().map(|r| r[1]).collect::<Vec<_>>();
        let first = Cell::new(0);
        let (fr, ft, fc) = (Role::Follower, Term(3), Index(40));

        let pre = Out::<C> { messages: vec![vote(0, 4, true), vote(2, 4, true)], ..Default::default() };
        let rows = elect_rows((fr, ft, fc), (Role::Follower, Term(3), Index(40)), &pre, &first, 100);
        assert_eq!(kinds(&rows), [Elect::VoteSent as u64; 2]);
        assert!(rows.iter().all(|r| r[5] == 1 && r[2] == 4 && r[4] == 40));

        let real = Out::<C> { messages: vec![vote(0, 4, false)], ..Default::default() };
        let rows = elect_rows((fr, ft, fc), (Role::Candidate, Term(4), Index(40)), &real, &first, 200);
        assert_eq!(kinds(&rows), [Elect::Role as u64, Elect::VoteSent as u64]);
        assert_eq!(rows[0][2..], [4, 0, 0, 1]);

        let won = Out::<C> { resolutions: vec![Resolution::Campaign(Verdict::Won)], ..Default::default() };
        let rows = elect_rows((Role::Candidate, Term(4), fc), (Role::Leader, Term(4), Index(40)), &won, &first, 300);
        assert_eq!(kinds(&rows), [Elect::Role as u64, Elect::CampaignEnded as u64]);
        assert_eq!((rows[0][4], rows[0][5], rows[1][5]), (1, 2, 1));

        let none = Out::<C>::default();
        let lead = (Role::Leader, Term(4), Index(40));
        let rows = elect_rows(lead, (Role::Leader, Term(4), Index(41)), &none, &first, 400);
        assert_eq!(kinds(&rows), [Elect::FirstCommit as u64]);
        assert_eq!((rows[0][0], rows[0][2], rows[0][4]), (400, 4, 41));
        assert!(
            elect_rows((Role::Leader, Term(4), Index(41)), (Role::Leader, Term(4), Index(42)), &none, &first, 500)
                .is_empty()
        );
        assert!(elect_rows((fr, Term(5), fc), (Role::Follower, Term(5), Index(41)), &none, &first, 600).is_empty());
        let rows = elect_rows((Role::Leader, Term(6), fc), (Role::Leader, Term(6), Index(41)), &none, &first, 700);
        assert_eq!(kinds(&rows), [Elect::FirstCommit as u64]);
    }

    /// An alarm whose invocation never ran (canceled) is re-armed once it is overdue, not left as the sooner one.
    /// F74's loop lag: a stopped ticker reads 0; a running one reads the larger of its last wake's lateness and how
    /// overdue its pending tick is, so a stall shows while it lasts, not only after the tick that ends it.
    #[test]
    fn loop_lag_reads_the_pending_ticks_overdue_time_or_the_last_wakes() {
        use super::loop_lag;
        assert_eq!(loop_lag(5_000, 300, 0), 0, "stopped ticker");
        assert_eq!(loop_lag(1_050, 0, 1_100), 0, "pending tick not due");
        assert_eq!(loop_lag(1_050, 40, 1_100), 40, "last wake was 40 ms late");
        assert_eq!(loop_lag(3_100, 40, 1_100), 2_000, "pending tick 2 s overdue");
    }

    /// F74's inbound trace: arrivals are numbered 1, 2, 3, ... with none skipped, whatever order handlers end in; each
    /// carries its route's code; and `open` counts the handlers not yet ended, this one included.
    #[test]
    fn inbound_rows_number_every_arrival_in_order_and_count_the_open_handlers() {
        use super::{Inbound, ROUTES};
        let log = Inbound::default();
        let (a, open_a, ga) = log.enter("/raft");
        let (b, open_b, gb) = log.enter("/suspect");
        drop(ga);
        let (c, open_c, gc) = log.enter("/nope");
        drop(gb);
        drop(gc);
        let (d, open_d, _gd) = log.enter("/read");
        let seq = |i: u64| i / 64;
        let code = |i: u64| (i % 64) as usize;
        assert_eq!([a, b, c, d].map(seq), [1, 2, 3, 4], "arrival numbers, contiguous and in order");
        assert_eq!(ROUTES[code(a) - 1], "/raft");
        assert_eq!(ROUTES[code(b) - 1], "/suspect");
        assert_eq!(code(c), 0, "an unlisted path is 0");
        assert_eq!(ROUTES[code(d) - 1], "/read");
        assert_eq!([open_a, open_b, open_c, open_d], [1, 2, 2, 1], "handlers open at each entry, itself included");
        assert!(ROUTES.len() < 64, "a code fits under the arrival number's factor");
    }

    /// m41-held's read source: a `/read` body names its query by what it selects, the old shape by `key`, and anything
    /// else is 0.
    #[test]
    fn a_read_names_its_query_by_what_it_selects() {
        use super::{READ_QUERIES, read_query_code};
        let name = |v: serde_json::Value| match read_query_code(&v) {
            0 => "other",
            c => READ_QUERIES[c as usize - 1],
        };
        assert_eq!(name(serde_json::json!({ "q": { "probe": true } })), "probe");
        assert_eq!(name(serde_json::json!({ "q": { "rows": { "api": 1 } }, "index": true })), "rows");
        assert_eq!(name(serde_json::json!({ "q": { "dep": true } })), "dep");
        assert_eq!(name(serde_json::json!({ "key": "k" })), "key");
        assert_eq!(name(serde_json::json!({ "q": {} })), "other");
        assert_eq!(name(serde_json::json!({ "q": { "nope": 1 } })), "other");
        // The edge's reads as it sends them, whose keys sort before `rows` (round 37).
        let tier = serde_json::json!({ "q": { "rows": true, "ranges": true, "packed": true } });
        assert_eq!(name(tier), "packed", "a whole-tier dump");
        let gate = serde_json::json!({ "q": { "rows": { "access": true, "agent": true }, "ranges": true } });
        assert_eq!(name(gate), "core", "the read gate's rows");
        let merge = serde_json::json!({ "q": { "rows": { "merge": 7 }, "ranges": true, "packed": true } });
        assert_eq!(name(merge), "rows", "a merge's rows");
        assert_eq!(name(serde_json::json!({ "q": { "ranges": true } })), "ranges", "the shard's ranges read");
        assert_eq!(name(serde_json::json!({ "q": { "refs": { "limit": 10 } } })), "refs");
        assert_eq!(name(serde_json::json!({ "q": { "lsRefs": { "reads": [], "head": true } } })), "lsRefs");
    }

    #[test]
    fn an_overdue_alarm_is_set_again() {
        use super::{ALARM_LATE_MS, rearm};
        assert!(rearm(0, 1_000, 900), "none armed");
        assert!(rearm(2_000, 1_000, 900), "sooner");
        assert!(!rearm(1_000, 2_000, 900), "a sooner one is armed and not yet due");
        assert!(!rearm(1_000, 2_000, 1_000 + ALARM_LATE_MS), "just due: its invocation may still come");
        assert!(rearm(1_000, 9_000, 1_001 + ALARM_LATE_MS), "overdue: lost");
    }

    /// The site's own shard gets the larger page it asks for, never past the local cap or under the standard page; a
    /// remote read keeps the standard page whatever it asks, so it still answers within the lookup's CALL. While the
    /// member serves other reads the local page is at most 1 MB, and an idle member serves the full 4 MB.
    #[test]
    fn only_a_local_entries_read_gets_a_larger_page() {
        use super::entries_page_bytes as page;
        use super::{ENTRIES_BUSY_PAGE_BYTES as BUSY, ENTRIES_LOCAL_PAGE_BYTES_MAX as MAX, ENTRIES_PAGE_BYTES as STD};
        assert_eq!(page(true, Some(2_000_000), 0), 2_000_000);
        assert_eq!(page(true, Some(u64::MAX), 0), MAX);
        assert_eq!(page(true, Some(1), 0), STD);
        assert_eq!(page(true, None, 0), STD);
        assert_eq!(page(false, Some(2_000_000), 0), STD);
        assert_eq!(page(true, Some(u64::MAX), 1), BUSY, "one read in flight already caps the page");
        assert_eq!(page(true, Some(800_000), 5), 800_000, "a page under the busy cap is not raised");
        assert_eq!(page(false, Some(u64::MAX), 5), STD);
    }

    use std::cell::Cell;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// LM-1 round 10's queue split: the wait for the entries ahead of the command, then its own entry's cut, dep reads
    /// and decide, summing to its queue time. A command admitted after the last apply waited for no slot.
    #[test]
    fn a_queue_splits_at_the_apply_that_freed_its_slot() {
        use super::queue_split;
        assert_eq!(queue_split(1_000, 1_300, 1_420), (300, 120), "waited 300 ms for the slot, then 120 ms of its own");
        assert_eq!(queue_split(1_000, 900, 1_150), (0, 150), "admitted after the last apply: no slot wait");
        assert_eq!(queue_split(1_000, 1_500, 1_150), (150, 0), "an apply later than the append clamps to it");
        let (slot, post) = queue_split(7, 40, 100);
        assert_eq!(slot + post, 93, "the split sums to the queue time");
    }

    /// cut-trace: a busy leader's member clock leads its own by up to seconds, and a command's admission time is its
    /// own. The queue and its split take both ends from that clock, so a member clock 1,000 ms ahead changes nothing.
    #[test]
    fn the_queue_split_reads_the_clock_its_admission_read() {
        use super::{Clocks, Stats};
        let mut s = Stats::default();
        // Admitted at 1,000 on the isolate's clock; the slot freed at 1,300 and the entry left at 1,420, while the
        // member clock read 1,000 ms ahead of it at both.
        s.applied_at(Clocks { member: 2_300, raw: 1_300 });
        s.queued(1_000, Clocks { member: 2_420, raw: 1_420 });
        assert_eq!(s.queue_ms, [420], "the queue mixed the member clock into a time taken on the isolate's");
        assert_eq!((s.queue_slot_ms[0], s.queue_post_ms[0]), (300, 120), "the split mixed the two clocks");
        // Admitted after the last apply on the isolate's clock, though the member clock had read later: no slot wait.
        s.applied_at(Clocks { member: 3_000, raw: 1_900 });
        s.queued(2_000, Clocks { member: 3_150, raw: 2_150 });
        assert_eq!((s.queue_ms[1], s.queue_slot_ms[1], s.queue_post_ms[1]), (150, 0, 150));
    }

    /// cut-trace: `tests/m0/cuttrace/cuts.py` unpacks kind 29's columns by these bit positions.
    #[test]
    fn a_cut_row_packs_its_note_where_the_trace_reader_looks() {
        let n = graftyard_raft::CutNote {
            taken: [1, 2, 3, 4_097],
            left: [5, 0, 7, 70_000],
            soft_bytes: 99_000,
            bytes: 230_000,
            tokens: 41.7,
            held: 2,
            in_flight: true,
            end: graftyard_raft::CutEnd::AgentTokens,
        };
        let (peer, batch, index, ms) = super::cut_row(&n);
        assert_eq!(peer, 3 + 32 + 64);
        assert_eq!(batch, 1 | 2 << 12 | 3 << 24 | 4_095 << 36, "taken saturates at 12 bits");
        assert_eq!(index, 5 | 7 << 26 | 8_191 << 39, "left saturates at 13 bits");
        assert_eq!((ms & 0x1f_ffff, (ms >> 21) & 0x1f_ffff, ms >> 42), (99_000, 230_000, 41));
        // A cut with one Human command, ENTRY_MAX soft bytes, and bytes and tokens past what a row holds (they
        // saturate): each column passes through a JS number (f64) and reads back exactly, the Human count too (B0-29
        // run 5's rows rounded it away).
        let full = graftyard_raft::CutNote {
            taken: [1, 0, 0, 9],
            soft_bytes: graftyard_raft::ENTRY_MAX,
            bytes: 3 * graftyard_raft::ENTRY_MAX,
            tokens: 1e9,
            ..n
        };
        let (_, batch, index, ms) = super::cut_row(&full);
        for v in [batch, index, ms] {
            assert_eq!(v as f64 as u64, v, "{v} does not survive a JS number");
        }
        assert_eq!((batch & 0xfff, batch >> 36), (1, 9), "one Human command and nine Agent ones");
        let back = |sh: u32| (ms >> sh) & 0x1f_ffff;
        assert_eq!((back(0), back(21), ms >> 42), (graftyard_raft::ENTRY_MAX as u64, (1 << 21) - 1, 2_047));
    }

    /// LM-1 round 12's confirm split: each transaction lands in its size group by its bytes (a 100 KB one is not
    /// under 100 KB), and `storage` is `sync` less the turn wait, per sample, not p99(sync) less p99(turn).
    #[test]
    fn a_confirm_split_groups_by_bytes_and_takes_the_turn_out_of_storage() {
        use super::{ConfirmSplit, confirm_split_summary};
        let c = |sync, turn, bytes| ConfirmSplit { gate: 3, sync, turn, stmts: 2, bytes };
        let v = confirm_split_summary(&[c(40, 30, 99_999), c(900, 10, 100_000), c(50, 45, 2_000_000)]);
        assert_eq!(v["lt100k"]["n"], 1);
        assert_eq!(v["lt100k"]["storage"]["p99"], 10);
        assert_eq!(v["lt1m"]["n"], 1, "100,000 bytes is the second group");
        assert_eq!(v["lt1m"]["storage"]["p99"], 890, "a storage-bound confirm");
        assert_eq!(v["lt1m"]["turn"]["p99"], 10);
        assert_eq!(v["ge1m"]["storage"]["p99"], 5, "a turn-bound confirm");
        let two = confirm_split_summary(&[c(100, 90, 1), c(80, 0, 2)]);
        assert_eq!(two["lt100k"]["storage"]["p99"], 80, "per sample: max(10, 80), not 100 - 90");
    }

    /// LM-1 round 14: an Agent command is priced without its inline pack and a Human one with all its bytes, and the
    /// summary's sums show which commands carry the mean.
    #[test]
    fn admitted_bytes_price_an_agent_command_without_its_pack() {
        use graftyard_raft::admit::AdmitClass as A;
        let mut s = super::Stats::default();
        s.admitted_bytes(A::Agent, 41_000, 40_000);
        s.admitted_bytes(A::Human, 41_000, 40_000);
        s.admitted_bytes(A::Agent, 300, 0);
        let v = super::admitted_bytes_summary(&s.admitted_bytes);
        assert_eq!(v["agent"]["n"], 2);
        assert_eq!(v["agent"]["sum"], 1_300);
        assert_eq!(v["agent"]["sumOver10k"], 0);
        assert_eq!(v["human"]["sum"], 41_000, "a human command's pack is priced");
        assert_eq!(v["human"]["sumOver10k"], 41_000);
    }

    /// A large transaction's mix names each statement's verb and table, quoted or not, largest bytes first.
    #[test]
    fn a_big_transaction_names_its_tables() {
        use worker::SqlStorageValue as V;
        assert_eq!(super::stmt_kind("INSERT OR REPLACE INTO log (idx, term) VALUES (?, ?)"), "INSERT log");
        assert_eq!(super::stmt_kind("DELETE FROM \"core_t3_pr\" WHERE k0 = ?"), "DELETE core_t3_pr");
        assert_eq!(super::stmt_kind("INSERT INTO request (rid) VALUES (?)"), "INSERT request");
        let stmts = vec![
            ("INSERT INTO request (rid) VALUES (?)", vec![V::Blob(vec![0; 10])]),
            ("INSERT OR REPLACE INTO log (idx) VALUES (?)", vec![V::Blob(vec![0; 500])]),
            ("INSERT INTO request (rid) VALUES (?)", vec![V::Blob(vec![0; 20])]),
        ];
        assert_eq!(
            super::stmt_mix(&stmts),
            vec![("INSERT log".to_string(), 1, 500), ("INSERT request".to_string(), 2, 30)]
        );
    }

    /// The bytes a transaction carries: blobs and text by length, other values 8 each.
    #[test]
    fn a_transaction_counts_its_argument_bytes() {
        use worker::SqlStorageValue as V;
        let stmts = vec![("a", vec![V::Blob(vec![0; 1_000]), V::Integer(7)]), ("b", vec![V::String("xyz".into())])];
        assert_eq!(super::stmt_bytes(&stmts), (2, 1_011));
    }

    /// `/stats`' `testMerges`: first attempts' asks and sheds add up, and the shard's latest counts are kept, not
    /// replaced by an attempt that asked for no merge.
    #[test]
    fn first_attempts_add_their_test_merges_to_the_leaders_stats() {
        let mut s = super::Stats::default();
        let shard = serde_json::json!({ "computed": 3, "shed": 1 });
        s.test_merges(&serde_json::json!({ "asked": 4, "shed": 1, "budget": shard }));
        s.test_merges(&serde_json::json!({ "asked": 2, "shed": 2, "budget": null }));
        s.test_merges(&serde_json::json!({ "skipped": 9 }));
        assert_eq!((s.test_merge_asks, s.test_merge_shed), (6, 3));
        assert_eq!(s.test_merges, Some(shard));
    }

    /// Reads of the same query granted in one input share one built answer, and a different query builds its own.
    #[test]
    fn reads_granted_together_share_one_answer_per_query() {
        use super::{ReadBody, shared_answer};
        let mut built = std::collections::HashMap::new();
        let mut builds = 0;
        let mut make = |v: &str| {
            builds += 1;
            ReadBody::Json(v.as_bytes().to_vec().into())
        };
        let a = serde_json::json!({ "refs": { "after": "YQ" } });
        let (first, hit) = shared_answer(&mut built, &a, 4, || make("1"));
        assert!(!hit);
        let (again, hit) = shared_answer(&mut built, &a, 4, || make("2"));
        assert!(hit);
        assert_eq!(again, first);
        let (ReadBody::Json(x), ReadBody::Json(y)) = (&first, &again) else { panic!("json") };
        assert!(std::rc::Rc::ptr_eq(x, y), "one copy, not one per read");
        let b = serde_json::json!({ "refs": { "after": "Yg" } });
        let (other, hit) = shared_answer(&mut built, &b, 4, || make("3"));
        assert!(!hit);
        assert_eq!(other, ReadBody::Json(b"3".to_vec().into()));
        // A read of the same query with a newer floor does not take an answer built for an older one.
        let (newer, hit) = shared_answer(&mut built, &a, 5, || make("4"));
        assert!(!hit, "another floor is another answer");
        assert_eq!(newer, ReadBody::Json(b"4".to_vec().into()));
        drop(make);
        assert_eq!(builds, 3);
    }

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
