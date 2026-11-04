//! Test-only fault injection, compiled in only with the `test-faults` feature, which product builds leave off.
//!
//! - Kills for BF-12 and BF-13, set per command by the edge's first send (`Kill`). `after_quorum`: the command's
//!   ticket resolved on the commit-advance path; the input's writes are made durable and nothing it produced is
//!   released. `after_quorum_unsynced`: the same point, with the input's writes dropped too. `before_commit`: the entry
//!   carrying the command was just appended; its writes and outputs are dropped, so the entry is lost.
//!   `before_commit_sent`: the entry is made durable and sent to the followers, and the leader dies before it hears
//!   any answer. `before_commit_early`: only the entry's AppendEntries leave, before the write, which is never made.
//!   Each writes a `fault` row (the original outcome, or the entry's index, term, and commands), waits for it to be
//!   durable, and calls `ctx.abort()`.
//! - The call-fault plan of the 24-hour linearizability run (`tests/m1/lin`), read once per isolate from the
//!   script's `GRAFTYARD_FAULTS` variable, as Milestone 0's B0-11 ran it: each member-to-member call fails before it
//!   leaves with `fail_call`, and each AppendEntries reply is dropped after the callee ran it with
//!   `drop_append_reply`. `/transfer` with `abandon` drops the transfer's `TimeoutNow`, so the leader abandons it
//!   when the LEASE runs out.
//! - Per-isolate counters of every injected fault (`/isolate`), with a random id drawn when the isolate first
//!   counts, so a run can say which isolates injected what (B0-11's carried note).
//! - BM-1's site fault (`SiteFault`): every call to or from one site's member class fails, as Milestone 0's B0-03
//!   simulated region loss (`tests/m0/src/faults.ts`, rules `">a:"` and `"a>"`). The plan is kept in each site's
//!   `Registry` object `faults` (`/site_fault`, which the edge's `/_bed/fault` sets) and reread by each isolate at most
//!   once a second, so it starts and ends mid-run with no new version. A member on the faulted site answers every
//!   Raft and edge call with 503 (`site_down`) and fails each call it sends before it leaves, reporting it with
//!   `call_failed`, so the leader's `Health` window sees every call fail.

use std::cell::RefCell;
use std::collections::BTreeMap;

use graftyard_raft::{Index, RequestId, Term};
use serde::Deserialize;
use serde_json::{Value, json};
use worker::{Env, js_sys};

pub const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS fault (seq INTEGER PRIMARY KEY AUTOINCREMENT, rid TEXT NOT NULL, mode TEXT NOT NULL, idx INTEGER NOT NULL, term INTEGER NOT NULL, result TEXT NOT NULL, at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS dup (rid TEXT NOT NULL, idx INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS deps_log (seq INTEGER PRIMARY KEY AUTOINCREMENT, idx INTEGER NOT NULL, at INTEGER NOT NULL, reads TEXT NOT NULL)",
    // BF-14: every `/commit` that reached this member, with its role then and what it answered.
    "CREATE TABLE IF NOT EXISTS arrival (seq INTEGER PRIMARY KEY AUTOINCREMENT, rid TEXT NOT NULL, at INTEGER NOT NULL, role TEXT NOT NULL, fault TEXT, answer TEXT NOT NULL)",
    "CREATE INDEX IF NOT EXISTS arrival_at ON arrival (at)",
];

/// BF-14's receipt faults, set per command by the edge's first send: the leader's reply leaves only after
/// COMMIT_RECEIPT has passed (`delay`), or is replaced by a 503 once the command was admitted (`drop`). Either way the
/// edge sees a missed receipt for a command the leader holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receipt {
    Delay,
    Drop,
}

impl Receipt {
    pub fn parse(s: &str) -> Option<Receipt> {
        match s {
            "delay" => Some(Receipt::Delay),
            "drop" => Some(Receipt::Drop),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Receipt::Delay => "delay",
            Receipt::Drop => "drop",
        }
    }
}

/// How long a `delay` holds the reply: past the edge's COMMIT_RECEIPT of 2 s.
pub const RECEIPT_DELAY_MS: u64 = 2_600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kill {
    AfterQuorum,
    AfterQuorumUnsynced,
    BeforeCommit,
    BeforeCommitSent,
    /// The leader's AppendEntries for the entry left before its own write, and the object reset before that write
    /// was made: the followers may hold an entry the leader lost.
    BeforeCommitEarly,
}

impl Kill {
    pub fn parse(s: &str) -> Option<Kill> {
        match s {
            "after_quorum" => Some(Kill::AfterQuorum),
            "after_quorum_unsynced" => Some(Kill::AfterQuorumUnsynced),
            "before_commit" => Some(Kill::BeforeCommit),
            "before_commit_sent" => Some(Kill::BeforeCommitSent),
            "before_commit_early" => Some(Kill::BeforeCommitEarly),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kill::AfterQuorum => "after_quorum",
            Kill::AfterQuorumUnsynced => "after_quorum_unsynced",
            Kill::BeforeCommit => "before_commit",
            Kill::BeforeCommitSent => "before_commit_sent",
            Kill::BeforeCommitEarly => "before_commit_early",
        }
    }
    /// The input's writes are made durable before the object resets.
    pub fn keeps_writes(self) -> bool {
        matches!(self, Kill::AfterQuorum | Kill::BeforeCommitSent)
    }
    pub fn at_append(self) -> bool {
        matches!(self, Kill::BeforeCommit | Kill::BeforeCommitSent | Kill::BeforeCommitEarly)
    }
}

pub struct Fault {
    pub rid: RequestId,
    pub kill: Kill,
    pub down_ms: u64,
    pub index: Index,
    pub term: Term,
    pub result: Value,
}

/// The run's call faults, from `GRAFTYARD_FAULTS`. Unset means none.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub struct Plan {
    #[serde(default)]
    pub fail_call: f64,
    #[serde(default)]
    pub drop_append_reply: f64,
}

struct Isolate {
    id: String,
    since: u64,
    plan: Option<Plan>,
    rolls: u64,
    counts: BTreeMap<String, u64>,
}

thread_local! {
    static ISOLATE: RefCell<Isolate> = RefCell::new(Isolate {
        id: format!("{:012x}", (js_sys::Math::random() * 281_474_976_710_656.0) as u64),
        since: crate::now_ms(),
        plan: None,
        rolls: 0,
        counts: BTreeMap::new(),
    });
}

fn plan(env: &Env) -> Plan {
    ISOLATE.with(|i| {
        let mut i = i.borrow_mut();
        *i.plan.get_or_insert_with(|| {
            env.var("GRAFTYARD_FAULTS").ok().and_then(|v| serde_json::from_str(&v.to_string()).ok()).unwrap_or_default()
        })
    })
}

/// Counts one injected fault in this isolate.
pub fn count(kind: &str) {
    ISOLATE.with(|i| *i.borrow_mut().counts.entry(kind.to_owned()).or_insert(0) += 1);
}

fn roll(rate: f64) -> bool {
    ISOLATE.with(|i| i.borrow_mut().rolls += 1);
    rate > 0.0 && js_sys::Math::random() < rate
}

/// A member-to-member call to site `to` fails before it leaves.
pub fn fail_call(env: &Env, to: u8) -> bool {
    let hit = roll(plan(env).fail_call);
    if hit {
        count(&format!("fail_call>{to}"));
    }
    hit
}

/// The reply to an AppendEntries call to site `to` is lost after the callee ran it.
pub fn drop_append_reply(env: &Env, to: u8) -> bool {
    let hit = roll(plan(env).drop_append_reply);
    if hit {
        count(&format!("drop_append_reply>{to}"));
    }
    hit
}

/// BM-1's site fault: site `site`'s member class fails every call from `from` until `until` (epoch ms), or with
/// `rate` below 1 that share of them, each call rolled on its own (the gray fault).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, serde::Serialize)]
pub struct SiteFault {
    pub site: Option<u8>,
    #[serde(default)]
    pub from: u64,
    #[serde(default)]
    pub until: u64,
    #[serde(default)]
    pub rate: Option<f64>,
}

/// How long an isolate keeps the plan it read.
const SITE_FAULT_TTL_MS: u64 = 1_000;

struct Cached {
    plan: SiteFault,
    fetched: u64,
    refreshing: bool,
}

thread_local! {
    static SITE_FAULT: RefCell<Option<Cached>> = const { RefCell::new(None) };
}

async fn read_site_fault(env: &Env) -> Option<SiteFault> {
    let site = crate::SiteEnv::read(env).ok()?;
    let stub = site.registry(env, site.id, "faults").ok()?;
    let mut r = stub.fetch_with_str("https://registry/site_fault").await.ok()?;
    r.json::<SiteFault>().await.ok()
}

fn keep(plan: Option<SiteFault>) {
    SITE_FAULT.with(|c| {
        let mut c = c.borrow_mut();
        let old = c.as_ref().map(|c| c.plan).unwrap_or_default();
        *c = Some(Cached { plan: plan.unwrap_or(old), fetched: crate::now_ms(), refreshing: false });
    });
}

/// Whether this script's site is failing every member call now. The first call in an isolate waits for the plan; a
/// stale plan is reread in the background, so the plan's object never sits on the measured path.
pub async fn site_down(env: &Env) -> bool {
    let Ok(me) = crate::SiteEnv::read(env).map(|s| s.id.0) else { return false };
    let now = crate::now_ms();
    let state = SITE_FAULT
        .with(|c| c.borrow().as_ref().map(|c| (c.plan, now >= c.fetched + SITE_FAULT_TTL_MS && !c.refreshing)));
    let plan = match state {
        None => {
            keep(read_site_fault(env).await);
            SITE_FAULT.with(|c| c.borrow().as_ref().map(|c| c.plan).unwrap_or_default())
        }
        Some((plan, stale)) => {
            if stale {
                SITE_FAULT.with(|c| {
                    if let Some(c) = c.borrow_mut().as_mut() {
                        c.refreshing = true;
                    }
                });
                let env = env.clone();
                worker::wasm_bindgen_futures::spawn_local(async move { keep(read_site_fault(&env).await) });
            }
            plan
        }
    };
    plan.site == Some(me) && plan.from <= now && now < plan.until && plan.rate.is_none_or(roll)
}

fn wasm_mem_mb() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return core::arch::wasm32::memory_size::<0>() as f64 * 65_536.0 / 1e6;
    #[cfg(not(target_arch = "wasm32"))]
    0.0
}

/// This isolate's counters, for `/isolate`.
pub fn isolate_json(env: &Env) -> Value {
    let p = plan(env);
    ISOLATE.with(|i| {
        let i = i.borrow();
        json!({
            "isolate": i.id,
            "since": i.since,
            // The isolate's wasm linear memory, which only grows: a member restore or a large read shows here first.
            "wasmMemMb": wasm_mem_mb(),
            "plan": { "failCall": p.fail_call, "dropAppendReply": p.drop_append_reply },
            "rolls": i.rolls,
            "counts": i.counts,
            "siteFault": SITE_FAULT.with(|c| c.borrow().as_ref().map(|c| json!({ "plan": c.plan, "fetched": c.fetched }))),
        })
    })
}
