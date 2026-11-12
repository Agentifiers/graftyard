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
//! - LF-13's member sever (`Host::severed_until`, member route `/sever`): one member fails as the site fault fails a
//!   whole member class, set and lifted in that object at once. The site fault takes up to a second per
//!   isolate to start and end and fails every group on the site, so it cannot place a follower's last entry before a
//!   leadership transfer; this can (`crates/raft/tests/lf13.rs`, the new leader's first round leaves a gap).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

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
    // BF-20's snapshot hold and one-shot force (`crate::snap`, `held` and `take_force`).
    "CREATE TABLE IF NOT EXISTS snap_hold (one INTEGER PRIMARY KEY)",
    "CREATE TABLE IF NOT EXISTS snap_force (tier INTEGER PRIMARY KEY)",
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
    /// B0-07's gray site: a hit call hangs this long before it fails, instead of failing at once, as Milestone 0's
    /// `hang` rule did (`tests/m0/src/faults.ts`). Calls past CALL_DEADLINE are then deadline misses.
    #[serde(default)]
    pub hang_ms: Option<u64>,
}

/// A `SiteFault.site` naming every site at once.
pub const ALL_SITES: u8 = 255;

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

/// Whether this call of this script's site fails now, and after how long: `Some(0)` at once, `Some(ms)` after
/// hanging `ms` (the plan's `hang_ms`). The first call in an isolate waits for the plan; a stale plan is reread in the
/// background, so the plan's object never sits on the measured path.
pub async fn site_fault(env: &Env) -> Option<u64> {
    let Ok(me) = crate::SiteEnv::read(env).map(|s| s.id.0) else { return None };
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
    // Site 255 is every site: L0-12's stall, every member running with every member call blackholed.
    let hit = (plan.site == Some(me) || plan.site == Some(ALL_SITES))
        && plan.from <= now
        && now < plan.until
        && plan.rate.is_none_or(roll);
    hit.then_some(plan.hang_ms.unwrap_or(0))
}

/// B0-07's stand-in for the `Release` group's site modes (`design/DESIGN.md`, Shape, `SiteMode`), which the hosted
/// code does not run yet: the drained sites and the sites under the first day's 1% lead limit, kept in each site's
/// `Registry` object `faults` (`/site_modes`, which the test edge sets for every site) and reread by each isolate at
/// most once a second in the background. The test runner, standing in for `graft-ops`, writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct SiteModes {
    #[serde(default)]
    pub drained: Vec<u8>,
    #[serde(default)]
    pub limited: Vec<u8>,
}

impl SiteModes {
    /// The sites whose members may not lead `chain`: drained ones, and lead-limited ones unless `chain` is in their
    /// 1% (an FNV-1a hash of its text form, mod 100, is 0) [inference: the design names no hash].
    pub fn may_not_lead(&self, chain: &str) -> BTreeSet<u8> {
        let h = chain.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        let limited = self.limited.iter().filter(|_| h % 100 != 0);
        self.drained.iter().chain(limited).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::SiteModes;

    #[test]
    fn a_lead_limit_spares_one_percent_of_chains_and_a_drain_spares_none() {
        let m = SiteModes { drained: vec![2], limited: vec![1] };
        let chains: Vec<String> = (0..10_000).map(|i| format!("repo:{i}")).collect();
        let spared = chains.iter().filter(|c| !m.may_not_lead(c).contains(&1)).count();
        assert!((60..=140).contains(&spared), "{spared} of 10,000 chains may lead on a limited site");
        assert!(chains.iter().all(|c| m.may_not_lead(c).contains(&2)));
        assert!(chains.iter().all(|c| !m.may_not_lead(c).contains(&0)));
    }
}

thread_local! {
    static MODES: RefCell<Option<(SiteModes, u64, bool)>> = const { RefCell::new(None) };
}

/// This isolate's site modes, never waiting: the first call starts the read and answers none, and a plan older than a
/// second is reread in the background.
pub fn site_modes(env: &Env) -> SiteModes {
    let now = crate::now_ms();
    let (modes, stale) = MODES.with(|m| match m.borrow().as_ref() {
        Some((v, at, refreshing)) => (v.clone(), now >= at + SITE_FAULT_TTL_MS && !refreshing),
        None => (SiteModes::default(), true),
    });
    if stale {
        MODES.with(|m| {
            let mut m = m.borrow_mut();
            let (v, at, _) = m.take().unwrap_or((SiteModes::default(), 0, false));
            *m = Some((v, at, true));
        });
        let env = env.clone();
        worker::wasm_bindgen_futures::spawn_local(async move {
            let read = async {
                let site = crate::SiteEnv::read(&env).ok()?;
                let stub = site.registry(&env, site.id, "faults").ok()?;
                stub.fetch_with_str("https://registry/site_modes").await.ok()?.json::<SiteModes>().await.ok()
            };
            let got = read.await;
            MODES.with(|m| {
                let mut m = m.borrow_mut();
                let old = m.take().map(|x| x.0).unwrap_or_default();
                *m = Some((got.unwrap_or(old), crate::now_ms(), false));
            });
        });
    }
    modes
}

/// B0-07's stand-in for `SiteHealth` (`design/DESIGN.md`, Shape, `SiteHealth`): this isolate's member-to-member calls
/// to each voter site, and how many failed or missed CALL_DEADLINE, flushed about once a second to the `Registry`
/// object `health` on this member's own site, which keeps them per second (`/health`), while a site fault is set.
pub fn site_health(env: &Env, target: u8, ok: bool) {
    let now = crate::now_ms();
    // Counted only while a site fault plan is set, so other runs on a test install pay no health writes.
    if !SITE_FAULT.with(|c| c.borrow().as_ref().is_some_and(|c| c.plan.site.is_some() && now < c.plan.until)) {
        return;
    }
    let flush = HEALTH.with(|h| {
        let mut h = h.borrow_mut();
        let c = h.0.entry(target).or_insert((0, 0));
        c.0 += 1;
        c.1 += u64::from(!ok);
        if now < h.1 + 1_000 {
            return None;
        }
        h.1 = now;
        Some(std::mem::take(&mut h.0))
    });
    let Some(counts) = flush else { return };
    let env = env.clone();
    worker::wasm_bindgen_futures::spawn_local(async move {
        let _ = health_put(&env, None, &counts).await;
    });
}

/// Per target site: calls and failed, since the last flush; and the last flush's time.
type Counts = BTreeMap<u8, (u64, u64)>;

thread_local! {
    static HEALTH: RefCell<(Counts, u64)> = const { RefCell::new((BTreeMap::new(), 0)) };
}

/// Adds `counts` (target site to calls and failed) to the `health` object on `observer` (default: this site).
pub async fn health_put(env: &Env, observer: Option<u8>, counts: &Counts) -> worker::Result<()> {
    let site = crate::SiteEnv::read(env)?;
    let o = observer.map_or(site.id, graftyard_types::SiteId);
    let mut init = worker::RequestInit::new();
    let body = json!({ "counts": counts.iter().map(|(t, (c, m))| json!([t, c, m])).collect::<Vec<_>>() });
    init.with_method(worker::Method::Put).with_body(Some(body.to_string().into()));
    let req = worker::Request::new_with_init("https://registry/health", &init)?;
    site.registry(env, o, "health")?.fetch_with_request(req).await.map(|_| ())
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
