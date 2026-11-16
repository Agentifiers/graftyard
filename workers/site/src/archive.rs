//! The leader's `LOG_MAX_BYTES` archive alarm (`design/DESIGN.md`, Shape, `LOG_MAX_BYTES` and "Packs";
//! `build/BUILD-PLAN.md`, AM1-WP1, "the leader's archive alarm that calls `archive_due`"). The decisions are
//! `graftyard_git::log_archive`'s, tested in memory on real members; this module is its host:
//!
//! - **When.** Each alarm of a leading repository member, after the snapshot writer's work, starts one run unless one
//!   is in flight on this member. A run sums the stored log above the last archive at most once a minute
//!   (`SUM_EVERY_MS`), so an idle or small group pays one SQLite aggregate a minute, and nothing when not leading.
//! - **Promote.** A due range no `Promoted` covers is promoted first: this site's git cache shard of the network
//!   catches up and answers `/promote` for the SHA-256s of the range's packs, then `/validate`s the result, which
//!   `PackStore::place` puts in the placement's buckets; `Promoted` is recorded and the pack and the range's stored
//!   packs are marked `Committed`.
//! - **Archive.** The covered part of the range, in pieces of `ARCHIVE_RANGE_MAX` bytes, goes create-only to two
//!   buckets of the record's sites, and `Archived` is recorded. Members truncate once the key is in two buckets
//!   (`Host::truncate`), and an installing member or the fold audit reads the entries back (`crate::load`).
//! - **Idempotent.** Request ids come from the range and the object's digest, so a run re-sent after a lost answer
//!   or a new leadership is answered from the `request` row and records nothing twice.
//!
//! The run is spawned, not awaited, so the alarm's Raft tick is never held behind the shard or R2. A run drains: it
//! takes steps one after another until nothing is due or `DRAIN_FOR` has passed, and each step commits on its own, so
//! a run cut off anywhere keeps every step before it. A run that fails notes why in the member's alarms and is
//! retried on a later alarm. One step that outlasts `STEP_FOR` is abandoned; a fixed deadline on the whole run
//! (`RUN_FOR`, 120 s) stopped B0-30's archive for good once each promotion's validate went to a cold shard
//! (`tests/am1/results/b0-30.md`, "Verdict").

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use graftyard_git::log_archive::{self, ARCHIVE_RANGE_MAX, ARCHIVE_SLACK, ArchiveHost, PROMOTE_MAX, Run, Step, View};
use graftyard_git::{self as git, Bucket as _, PackIndex, PackRef, PutNew};
use graftyard_raft::{Format, Index, RequestId, Role};
use graftyard_types::{ChainId, Digest, NetworkId, SiteId, Timestamp, Ulid};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::{Host, blob, int, rows};
use crate::{SiteEnv, now_ms};

/// How often a leader sums its stored log's bytes to ask whether an archive is due [guess].
const SUM_EVERY_MS: u64 = 60_000;
// A log that passes the archive's trigger just after a sum grows one `SUM_EVERY_MS` more before a run starts: the
// trigger sits below the cap by more than that at the rate the band is sized for (`log_archive::ARCHIVE_SLACK`).
const _: () = assert!(
    graftyard_fold::LOG_MAX_BYTES / ARCHIVE_SLACK / 2
        > log_archive::DESIGN_LOG_BYTES_PER_S * (SUM_EVERY_MS + BUSY_MS as u64) / 1_000
);
/// After a run that did work, the next alarm comes this soon, so a due range drains piece by piece.
const BUSY_MS: i64 = 200;
/// A run starts no new step after this long [guess], so a member's isolate is not held by one drain for ever.
const DRAIN_FOR_MS: u64 = 90_000;
/// One step that has not finished in this long is abandoned [guess]: a shard's catch-up behind it saves its cursor
/// piece by piece, so the next step resumes the work. Over a push's 180 s, because a promotion waits on the same
/// catch-up a push would.
const STEP_FOR: std::time::Duration = std::time::Duration::from_secs(600);

thread_local! {
    /// Test installs: the cap and range size in place of `LOG_MAX_BYTES` and `ARCHIVE_RANGE_MAX`, for this isolate
    /// (`/bench_seed {"archiveCap": {"max": n, "range": n}}`), so a hosted smoke archives without 2 GiB of log.
    #[cfg(feature = "test-faults")]
    static CAP: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
    /// Members of this isolate with a run in flight, by object name.
    static RUNNING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Members of this isolate whose last run stopped with work left, by object name, and when (`leader_due`).
    static LEFT: RefCell<std::collections::HashMap<String, u64>> = RefCell::new(Default::default());
    /// When each member last summed its log, by object name.
    static SUMMED: RefCell<std::collections::HashMap<String, u64>> = RefCell::new(Default::default());
    /// Each member's drain run, by object name, kept across the runs that continue it after `DRAIN_FOR_MS`, so a run
    /// that passed the trigger archives down to the floor (`log_archive::Run`).
    static RUN: RefCell<std::collections::HashMap<String, Run>> = RefCell::new(Default::default());
}

/// Starts one run on a leading repository member, unless one is in flight or the log was summed under a minute ago
/// and the last run found nothing to do.
pub fn on_alarm(h: &Rc<Host<Repo>>) {
    let key = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        if !matches!(c.join.chain, ChainId::Repo(_)) {
            return;
        }
        c.join.me.do_name.to_string()
    };
    if RUNNING.with(|r| r.borrow().contains(&key)) {
        return;
    }
    let due = SUMMED.with(|s| s.borrow().get(&key).is_none_or(|at| now_ms() >= at + SUM_EVERY_MS));
    if !due {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(key.clone()));
    let w = Rc::downgrade(h);
    spawn_local(async move {
        let mut run = RUN.with(|r| r.borrow().get(&key).copied().unwrap_or_default());
        let r = match w.upgrade() {
            Some(h) => drain(&h, &mut run).await,
            None => Ok(Step::Idle),
        };
        RUN.with(|r| r.borrow_mut().insert(key.clone(), run));
        RUNNING.with(|r| r.borrow_mut().remove(&key));
        LEFT.with(|l| match &r {
            Ok(Step::Idle) => l.borrow_mut().remove(&key),
            _ => l.borrow_mut().insert(key.clone(), now_ms()),
        });
        let Some(h) = w.upgrade() else { return };
        match r {
            // Nothing due: the next sum waits a minute.
            Ok(Step::Idle) => {
                SUMMED.with(|s| s.borrow_mut().insert(key, now_ms()));
            }
            // The drain stopped at its budget with work left: the next run starts at once.
            Ok(_) => h.arm_in(BUSY_MS),
            // A failed run is retried on its own a minute on: an idle leader arms no other alarm.
            Err(e) => {
                SUMMED.with(|s| s.borrow_mut().insert(key, now_ms()));
                note(&h, format!("archive: {e}"));
                h.arm_in(SUM_EVERY_MS as i64);
            }
        }
    });
}

/// Whether this member's last archive run as leader, within `LEFT_FOR`, stopped with a range still due or failed:
/// the next leader's work, so a member that has heard no leader since acts for it (`crate::host::keep_led`). From
/// this isolate's memory, so it costs no log sum; a reset member forgets it until it leads again, and a new leader's
/// own runs take it over.
pub fn leader_due(h: &Rc<Host<Repo>>) -> bool {
    let Some(key) = h.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()) else { return false };
    LEFT.with(|l| l.borrow().get(&key).is_some_and(|at| now_ms() < at + LEFT_FOR_MS))
}

/// How long a run that left work counts for `leader_due` [guess]: well past one election, and short enough that a
/// former leader whose work a quiet new leader finished stops probing it.
const LEFT_FOR_MS: u64 = 600_000;

/// Steps until nothing is due, a step fails, or `DRAIN_FOR_MS` has passed: the last step's result.
async fn drain(h: &Rc<Host<Repo>>, r: &mut Run) -> std::result::Result<Step, String> {
    let began = now_ms();
    loop {
        let t0 = now_ms();
        let (s, took) =
            crate::host::within(STEP_FOR, run(h, r)).await.unwrap_or_else(|| Err("a step timed out".into()))?;
        if s == Step::Idle {
            return Ok(s);
        }
        // The step's wall time and its entry's commit, which the drain's throughput bound is built from.
        console_log!("archive {s:?} in {} ms, commit {took} ms", now_ms() - t0);
        h.stats.borrow_mut().archive_steps += 1;
        if now_ms() >= began + DRAIN_FOR_MS {
            return Ok(s);
        }
        // Paced by the step's own entry, not its shard or bucket time (`crate::host::PACE`).
        Delay::from(crate::host::paced(took)).await;
    }
}

fn note(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

/// One step of the run, and how long its entry took to commit.
async fn run(h: &Rc<Host<Repo>>, r: &mut Run) -> std::result::Result<(Step, u64), String> {
    h.settled().await;
    let sql = h.state.storage().sql();
    let s = SiteHost { h, sql, recorded_ms: std::cell::Cell::new(0) };
    let (max, range) = cap();
    let step = log_archive::step(&s, r, max, range, PROMOTE_MAX).await?;
    Ok((step, s.recorded_ms.get()))
}

/// `LOG_MAX_BYTES` and `ARCHIVE_RANGE_MAX`, or a test install's override.
fn cap() -> (u64, u64) {
    #[cfg(feature = "test-faults")]
    if let Some(c) = CAP.with(|c| c.get()) {
        return c;
    }
    (graftyard_fold::LOG_MAX_BYTES, ARCHIVE_RANGE_MAX)
}

/// Test installs: sets or clears this isolate's cap override, and forgets when each member last summed its log so
/// the next alarm asks at once.
#[cfg(feature = "test-faults")]
pub fn set_cap(v: &serde_json::Value) -> serde_json::Value {
    let cap = v["max"].as_u64().map(|m| (m, v["range"].as_u64().unwrap_or(ARCHIVE_RANGE_MAX)));
    CAP.with(|c| c.set(cap));
    SUMMED.with(|s| s.borrow_mut().clear());
    serde_json::json!({ "archiveCap": cap })
}

struct SiteHost<'a> {
    h: &'a Rc<Host<Repo>>,
    sql: SqlStorage,
    /// How long the step's `record` waited for its entry.
    recorded_ms: std::cell::Cell<u64>,
}

impl SiteHost<'_> {
    /// Promote-ahead's bound, past which `View::unpromoted` need not count further.
    fn ahead_bound(&self) -> u64 {
        cap().0 / log_archive::PROMOTE_AHEAD
    }

    fn chain(&self) -> std::result::Result<(ChainId, graftyard_types::PlacementRecord), String> {
        let core = self.h.core.borrow();
        let c = core.as_ref().ok_or("not joined")?;
        Ok((c.join.chain, c.join.record.clone()))
    }

    /// The record's sites' buckets.
    fn buckets(&self) -> std::result::Result<Vec<(SiteId, git::host::R2)>, String> {
        let (_, record) = self.chain()?;
        record
            .sites
            .iter()
            .map(|s| Ok((*s, git::host::R2(self.h.env.bucket(&format!("BUCKET_{}", s.0)).map_err(|e| e.to_string())?))))
            .collect()
    }
}

impl ArchiveHost for SiteHost<'_> {
    fn rollup(&self) -> bool {
        log_archive::rollup_enabled(self.h.env.var("GRAFTYARD_ROLLUP").ok().map(|v| v.to_string()).as_deref())
    }

    fn view(&self) -> Option<View> {
        let core = self.h.core.borrow();
        let c = core.as_ref().filter(|c| c.m.role() == Role::Leader)?;
        let fold = &c.m.group_fold().fold;
        let sum = |after: u64| -> Option<u64> {
            let q = "SELECT COALESCE(SUM(length(row)), 0) FROM log WHERE idx > ?";
            Some(rows(&self.sql, q, vec![crate::host::i(after)]).ok()?.first().and_then(|r| int(&r[0]).ok())? as u64)
        };
        let total = sum(fold.archived().map_or(0, |a| a.to.0))?;
        // Pack bytes, not row bytes, read only until past promote-ahead's bound, so the walk costs at most that much.
        let after = fold.promoted().map_or(0, |p| p.to.0);
        let ahead = self.ahead_bound();
        let q = "SELECT length(row), CASE WHEN length(row) < ? THEN row END FROM log WHERE idx > ? ORDER BY idx";
        let it = self
            .sql
            .exec(q, vec![crate::host::i(log_archive::SHORT_ROW), crate::host::i(after)])
            .ok()?
            .raw()
            .map_while(|r| {
                let r = r.ok()?;
                Some((int(&r[0]).ok()? as u64, blob(&r[1]).ok().map(|b| b.to_vec())))
            });
        let unpromoted = log_archive::pack_bytes_above(it, ahead);
        Some(View::of(c.join.chain, fold, c.m.state().commit, total, unpromoted))
    }

    fn rows(&self, after: Index) -> std::result::Result<log_archive::Rows<'_>, String> {
        // Read lazily, so the walk stops one row past the range without loading the log.
        let it = self
            .sql
            .exec("SELECT idx, row FROM log WHERE idx > ? ORDER BY idx", vec![crate::host::i(after.0)])
            .map_err(|e| e.to_string())?
            .raw();
        Ok(Box::new(it.map_while(|r| {
            let r = r.ok()?;
            Some((Index(int(&r[0]).ok()? as u64), blob(&r[1]).ok()?.to_vec()))
        })))
    }

    /// In a new invocation of this site script (`in_band`), not in this member: see `PROMOTE_PATH`.
    async fn promote(
        &self,
        sums: &[Digest],
        from: Index,
        to: Index,
        request: RequestId,
        warm: bool,
    ) -> std::result::Result<Option<PackRef>, String> {
        let (chain, record) = self.chain()?;
        let p = Promote { chain, record, sums: sums.to_vec(), request, range: Some((from.0, to.0)), warm, roll: false };
        in_band(&self.h.env, &p).await
    }

    /// In a new invocation of this site script (`in_band`), through the same placement path as a promotion. A roll is
    /// always warm, so it never starts a stopped shard (`decisions/DECISIONS.md`, `roll-up`, "Who writes it").
    async fn roll(
        &self,
        sums: &[Digest],
        from: Index,
        to: Index,
        request: RequestId,
    ) -> std::result::Result<Option<PackRef>, String> {
        let (chain, record) = self.chain()?;
        let p = Promote {
            chain,
            record,
            sums: sums.to_vec(),
            request,
            range: Some((from.0, to.0)),
            warm: true,
            roll: true,
        };
        in_band(&self.h.env, &p).await
    }

    /// One timer turn, so the member's inputs and its Raft tick run between pieces of a promotion's read.
    async fn pause(&self) {
        Delay::from(std::time::Duration::from_millis(1)).await;
    }

    async fn mark(&self, pack: &PackRef, index: Index) {
        let (Ok((chain, _)), Ok(raw)) = (self.chain(), self.buckets()) else { return };
        let ChainId::Repo(repo) = chain else { return };
        let refs: Vec<(SiteId, &git::host::R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
        let failed = git::mark_committed(&refs, NetworkId(repo.0), pack, chain, index).await;
        self.h.stats.borrow_mut().marker_failures += failed as u32;
    }

    /// The first two of the record's sites, as the bed's archive route put them; `Host::truncate` HEADs every
    /// site's bucket, so any two count.
    /// Both puts at once, from the same bytes: an archive step puts up to `ARCHIVE_PIECES` objects, and their puts are
    /// most of its time (`log_archive::PROMOTE_RANGES`).
    async fn put(&self, key: &str, bytes: &[u8], digest: Digest) -> std::result::Result<usize, String> {
        let buckets = self.buckets()?;
        let puts = buckets.iter().take(2).map(|(_, b)| b.put_new(key, bytes, digest, &[]));
        let mut held = 0;
        for r in futures_util::future::join_all(puts).await {
            match r.map_err(|e| e.0)? {
                PutNew::Created => held += 1,
                PutNew::Exists(Some(d)) if d == digest => held += 1,
                PutNew::Exists(_) => {}
            }
        }
        Ok(held)
    }

    async fn record(
        &self,
        request: RequestId,
        format: Format,
        events: Vec<git::repo::RepoEvent>,
    ) -> std::result::Result<Index, String> {
        let rx = self.h.wait_recorded(request);
        let t0 = now_ms();
        let r = self.h.input(|m, t| m.record(t, request, format, events)).map_err(|e| e.to_string())?;
        r.map_err(|e| crate::host::call_error_json(&e).to_string())?;
        self.h.served();
        let r = crate::host::within(std::time::Duration::from_secs(10), rx).await;
        self.recorded_ms.set(self.recorded_ms.get() + (now_ms() - t0));
        match r {
            Some(Ok(Ok(index))) => Ok(index),
            Some(Ok(Err(nl))) => Err(format!("not leader: {}", crate::host::nl_json(&nl))),
            Some(Err(_)) => Err("member reset".into()),
            None => Err("the record did not commit in 10 s".into()),
        }
    }
}

/// The path of a promotion on the site script's own fetch handler (`in_band`).
pub const PROMOTE_PATH: &str = "/archive/promote";

/// One promotion as the handler takes it: the range's pack checksums, under the leader's request id.
#[derive(serde::Serialize, serde::Deserialize)]
struct Promote {
    chain: ChainId,
    record: graftyard_types::PlacementRecord,
    sums: Vec<Digest>,
    request: RequestId,
    /// The range, which the pack's stored `.idx` names (`git::Place::idx_range`).
    #[serde(default)]
    range: Option<(u64, u64)>,
    /// Promote-ahead's, and every roll's: every shard call is a warm call (`git::caller`), so a stopped shard is
    /// skipped, not started.
    #[serde(default)]
    warm: bool,
    /// Ask the shard to roll these stored packs rather than promote a log range's packs.
    #[serde(default)]
    roll: bool,
}

/// Runs a promotion's shard calls and its placement in a new invocation of this site script, through its own `SITE`
/// binding, not in the leader member: the runtime charges an object's outbound calls to its newest incoming request,
/// and a cold shard's catch-up behind `/promote` read `/entries` back from this member one entry at a time, so each
/// read went one hop deeper until one failed "Subrequest depth limit exceeded" (merge-budget-4's trace `8c049c94`,
/// `tests/am1/results/hot-shards-3.md`), as `PrSync`'s first attempt did (`crate::prsync::in_band`).
async fn in_band(env: &Env, p: &Promote) -> std::result::Result<Option<PackRef>, String> {
    let body = serde_json::to_string(p).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.into()));
    let req = Request::new_with_init(&format!("https://site{PROMOTE_PATH}"), &init).map_err(|e| e.to_string())?;
    let mut r = env.service("SITE").map_err(|e| e.to_string())?.fetch_request(req).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    match r.status_code() {
        200 => serde_json::from_str(&text).map_err(|e| format!("promotion answered {text}: {e}")),
        s => Err(format!("promotion: {s} {text}")),
    }
}

/// The site script's fetch handler for `in_band`: the promotion's pack, placed, `null` when a warm promotion found every
/// shard stopped, or a 500 with the error. It runs under
/// `wait_until`, so a leader whose step timed out does not cut it off between the shard and the buckets.
pub async fn promote_handled(mut req: Request, env: Env, ctx: &Context) -> Result<Response> {
    let p: Promote = req.json().await?;
    let (tx, rx) = futures_channel::oneshot::channel();
    ctx.wait_until(async move {
        // LM-1 round 21: the promotion's size, time and this isolate's wasm memory around it, for G9's hosted rate and
        // the streamed promotion's memory bound (`tests/am1/results/lm1-memory.md`).
        let (t0, mem0) = (now_ms(), crate::load::wasm_mem_mb());
        let r = promote(&env, &p).await;
        let bytes = match &r {
            Ok(Some(PackRef::Stored { bytes, .. })) => *bytes,
            _ => 0,
        };
        console_log!(
            "{} {:?}{}: {} packs, {bytes} B in {} ms, wasm {mem0:.1} -> {:.1} MB{}",
            if p.roll { "roll" } else { "promotion" },
            p.range,
            if matches!(r, Ok(None)) { " skipped, every shard stopped" } else { "" },
            p.sums.len(),
            now_ms() - t0,
            crate::load::wasm_mem_mb(),
            r.as_ref().err().map(|e| format!(", failed: {e}")).unwrap_or_default()
        );
        let _ = tx.send(r);
    });
    match rx.await {
        Ok(Ok(pack)) => Response::from_json(&pack),
        Ok(Err(e)) => Response::error(e, 500),
        Err(_) => Response::error("promotion dropped", 500),
    }
}

async fn promote(env: &Env, p: &Promote) -> std::result::Result<Option<PackRef>, String> {
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    let ChainId::Repo(repo) = p.chain else { return Err("not a repository chain".into()) };
    let sums = p.sums.iter().map(|s| s.hex()).collect::<Vec<_>>();
    // `/promote` takes its JSON list; `/roll` takes the same oldest-first checksums one hex string per line.
    let body =
        if p.roll { sums.join("\n").into_bytes() } else { serde_json::to_vec(&sums).map_err(|e| e.to_string())? };
    // This site's shard first, then each other Full site's of the record while the one before answers anything
    // but a 200 or the pack's 422 (`git::promote_sites`, `git::promote_moves_on`): after a reset this site's shard
    // rehydrates the tail for half an hour, and every promotion meanwhile answered 503 warming, so archiving
    // stopped behind it (`tests/am1/results/hot-shards.md`).
    let raw: Vec<(SiteId, git::host::R2)> = p
        .record
        .sites
        .iter()
        .map(|s| Ok((*s, git::host::R2(env.bucket(&format!("BUCKET_{}", s.0)).map_err(|e| e.to_string())?))))
        .collect::<std::result::Result<_, String>>()?;
    let refs: Vec<(SiteId, &git::host::R2)> = raw.iter().map(|(s, b)| (*s, b)).collect();
    let store = git::PackStore { sites: &site.sites, buckets: &refs, copies: git::Copies::TwoRegions };
    let (mut last, mut skipped) = (String::new(), 0);
    let sites = git::promote_sites(&p.record, site.id);
    for (i, at) in sites.iter().copied().enumerate() {
        // The id is fresh per attempt: an attempt whose `Promoted` never committed leaves an unmarked pack, which
        // the sweeper deletes once its `request` row is still missing past `ORPHAN_AFTER`.
        let mut rand = [0u8; 10];
        rand.copy_from_slice(&p.request.0.0.to_be_bytes()[6..]);
        rand[0] ^= i as u8;
        let s = git::Streamed {
            network: NetworkId(repo.0),
            id: git::PackId(Ulid::new(Timestamp(now_ms()), rand)),
            meta: git::ObjectMeta { chain: p.chain, request: p.request },
            part: git::PART_SIZE,
        };
        match promote_on(env, &site, p, at, &body, i + 1 < sites.len(), &store, s).await {
            Ok(r) => return r.map(Some),
            Err((SKIPPED, _)) => skipped += 1,
            Err((status, e)) if git::promote_moves_on(status) => last = e,
            Err((_, e)) => return Err(e),
        }
    }
    if skipped == sites.len() { Ok(None) } else { Err(last) }
}

/// `/promote` and the `/validate` of its pack on one site's shard, the validate on the shard that built the pack. The
/// pack streams from the shard's `/promote` to the buckets in parts and to the validate (`PackStore::place_promoted`),
/// so this isolate holds two parts of it whatever its size, and a range whose first entry is over `PROMOTE_MAX`
/// promotes alone. Every pack comes from its own shard, so a shard that fails mid-pack is passed over as one that
/// failed before it: `Err((status, body))`, as `promote_moves_on` reads it. The inner `Err` is the buckets', the same
/// on every shard. `next`: another site's shard follows, so a shard that is rehydrating answers 503 `warming` at once.
#[allow(clippy::too_many_arguments)]
async fn promote_on(
    env: &Env,
    site: &SiteEnv,
    p: &Promote,
    at: SiteId,
    body: &[u8],
    next: bool,
    store: &git::PackStore<'_, git::host::R2>,
    s: git::Streamed,
) -> std::result::Result<std::result::Result<PackRef, String>, (u16, String)> {
    // A promotion's `wait`: a shard still catching up answers 503 "warming" after it and goes on in the background
    // (`git::promote_sites`), shorter when another site's shard follows (`git::promote_wait_s`). A roll takes no wait
    // or `next`: it is already warm, and its path otherwise follows the same calls.
    let path = if p.roll {
        "/roll?".to_string()
    } else {
        format!("/promote?wait={}&{}", git::promote_wait_s(next), if next { "next=1&" } else { "" })
    };
    let body = js_sys::Uint8Array::from(body).into();
    let pack = shard_call(env, site, p, at, &path, Some(body)).await?.stream().map_err(|e| (0, e.to_string()))?;
    let pack = futures_util::StreamExt::map(pack, |c| c.map_err(|e| e.to_string()));
    // `via=0`: the shard that built the pack validates it. Through `fanout` it went to a pool shard, whose
    // catch-up on a hot group is a rehydrate of gigabytes, and archiving stopped (`tests/am1/results/b0-30.md`).
    // `derived=1`: not a push, so it reports no `NetworkLoad` and its CPU stays out of `fanout`'s k (A31).
    let validate = |reader: git::PipeReader| async move {
        let path = format!("/validate?tipsn=0&via=0&derived=1&wait={}&", git::PROMOTE_WAIT_S);
        let chunks = futures_util::StreamExt::map(reader, |c| {
            c.map(|b| js_sys::Uint8Array::from(&b[..]).into()).map_err(|e| wasm_bindgen::JsValue::from_str(&e))
        });
        let body = wasm_streams::ReadableStream::from_stream(chunks).into_raw();
        let ix = shard(env, site, p, at, &path, Some(body.into())).await?;
        serde_json::from_slice::<PackIndex>(&ix).map_err(|e| (0, e.to_string()))
    };
    // Derived: a shard without `/idx` (the previous image) or a failed read stores the pack alone, as before.
    let idx =
        |sum: Digest| async move { shard(env, site, p, at, &format!("/idx?sha256={}&", sum.hex()), None).await.ok() };
    let range = p.range.map(|(f, t)| (Index(f), Index(t)));
    match store.place_promoted(s, pack, validate, idx, range).await {
        Err(e) => Err((0, format!("the shard's pack broke off: {e}"))),
        Ok(Ok(pack)) => Ok(Ok(pack)),
        Ok(Err(git::TeeError::Invalid(e))) => Err(e),
        Ok(Err(git::TeeError::Place(e))) => Ok(Err(e.to_string())),
    }
}

/// Site `at`'s git cache shard of the network, its answer's body: `shard_call`'s.
async fn shard(
    env: &Env,
    site: &SiteEnv,
    p: &Promote,
    at: SiteId,
    path: &str,
    body: Option<wasm_bindgen::JsValue>,
) -> std::result::Result<Vec<u8>, (u16, String)> {
    shard_call(env, site, p, at, path, body).await?.bytes().await.map_err(|e| (0, e.to_string()))
}

/// The status `shard_call` gives a warm call's `skipped` answer (no HTTP status is 1): the shard's container is stopped.
const SKIPPED: u16 = 1;

/// Site `at`'s git cache shard of the network, whose catch-up reads the group through the record: its answer, body
/// unread, or `Err((status, body))` for an answer other than 200, status 0 for a failed call and `SKIPPED` for a warm
/// call (`Promote::warm`) to a stopped shard.
async fn shard_call(
    env: &Env,
    site: &SiteEnv,
    p: &Promote,
    at: SiteId,
    path: &str,
    body: Option<wasm_bindgen::JsValue>,
) -> std::result::Result<Response, (u16, String)> {
    let fail = |e: String| (0, e);
    let ChainId::Repo(repo) = p.chain else { return Err(fail("not a repository chain".into())) };
    let region = site.sites.get(at).ok_or_else(|| fail(format!("site {} is not in GRAFTYARD_SITES", at.0)))?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", at.0))
        .and_then(|ns| ns.get_by_name_with_location_hint(&format!("git#{}#net:{}", at.0, repo.0.0), region.hint()))
        .map_err(|e| fail(e.to_string()))?;
    let rec = serde_json::to_string(&p.record).map_err(|e| fail(e.to_string()))?;
    let warm = if p.warm { "warm=1&" } else { "" };
    let url = format!("https://shard{path}{warm}repo={}&record={}", repo.0.0, js_sys::encode_uri_component(&rec));
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(body);
    let req = Request::new_with_init(&url, &init).map_err(|e| fail(e.to_string()))?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| fail(e.to_string()))?;
    if r.status_code() != 200 {
        return Err((
            r.status_code(),
            format!("shard {path}: {} {}", r.status_code(), r.text().await.unwrap_or_default()),
        ));
    }
    if r.headers().has("x-graft-skipped").unwrap_or(false) {
        return Err((SKIPPED, format!("shard {path}: stopped")));
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use graftyard_git::log_archive;
    use graftyard_git::repo::RepoEvent;
    use graftyard_git::{PackId, PackRef, Promoted};
    use graftyard_raft::{EntryBody, LogEntry, TierSet, codec};
    use graftyard_types::{ChainId, DbId, Digest, Index, RepoId, RequestId, SiteSet, Timestamp, Ulid};
    use smallvec::SmallVec;

    use crate::family::{Family, Repo};

    fn entry(i: u64, events: Vec<RepoEvent>) -> LogEntry<RepoEvent> {
        LogEntry {
            term: graftyard_raft::Term(1),
            index: Index(i),
            prev: Digest([0; 32]),
            format: graftyard_git::repo::REPO_FORMAT_V1,
            tiers: TierSet::CORE,
            at: Timestamp(1_000 * i),
            requests: SmallVec::new(),
            extend: None,
            body: EntryBody::Events(events),
        }
    }

    /// The `Archived` events the alarm records are the ones `Host::truncate` acts on: `Family::archived` reads each
    /// key and range, and each key names the object the alarm put, so a member HEADs the same key in the buckets.
    #[test]
    fn the_alarm_s_archived_events_are_the_ranges_members_truncate_through() {
        let rows: Vec<_> =
            (3..=5).map(|i| (Index(i), Timestamp(1_000 * i), codec::to_bytes(&entry(i, vec![])))).collect();
        let chain = ChainId::Repo(RepoId(DbId(9)));
        let (bytes, a) = log_archive::archive_object(chain, &rows).unwrap();
        assert!(bytes.len() > 3 * rows[0].2.len());
        let key = a.key.to_string();
        assert!(key.starts_with("archive/repo:9/"), "{key}");
        let rows2: Vec<_> =
            (6..=7).map(|i| (Index(i), Timestamp(1_000 * i), codec::to_bytes(&entry(i, vec![])))).collect();
        let (_, a2) = log_archive::archive_object(chain, &rows2).unwrap();
        let key2 = a2.key.to_string();
        // An entry that records several objects names each, in order, so a member truncates through each in turn.
        let r = Repo::archived(&entry(8, vec![RepoEvent::Archived(a), RepoEvent::Archived(a2)]));
        let got: Vec<_> = r.into_iter().map(|r| (r.key, r.from, r.to, r.to_at)).collect();
        assert_eq!(got, [(key, 3, 5, 5_000), (key2, 6, 7, 7_000)]);
    }

    /// The sweeper's `request` row for a `Promoted` names its stored pack, so a marker the alarm failed to put is
    /// repaired and the pack is never taken for an orphan.
    #[test]
    fn a_promoted_entry_s_request_row_names_its_pack() {
        let pack =
            PackRef::Stored { id: PackId(Ulid(7 << 80)), checksum: Digest([7; 32]), bytes: 70, sites: SiteSet(3) };
        let p = Promoted { from: Index(1), to: Index(5), pack: pack.clone() };
        let v = Repo::row_packs(&[RepoEvent::Promoted(p)]);
        let named: Vec<PackRef> = serde_json::from_value(v).unwrap();
        assert_eq!(named, vec![pack]);
        // A push's row still names its own pack alone.
        let push = RepoEvent::Refs { push: RequestId(Ulid(1)), actor: None, updates: vec![], pack: None };
        assert_eq!(Repo::row_packs(&[push]), serde_json::json!([]));
    }
}
