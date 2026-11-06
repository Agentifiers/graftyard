//! The `MergeQueue` consumer on a site script (`design/DESIGN.md`, Shape, "Merge queue"; `design/SHARED.md`, "Event
//! fan-out"), next to `crate::prsync` and on the same pattern. The pure half is `graftyard_git::queue_consumer`.
//!
//! - **Leader alarm.** A leading repository member runs a `Dispatcher<RefName>` from the committed `MergeQueue`
//!   cursor: each entry names the base branches it touched (`queue_consumer::touched`). A batch's first attempt runs in
//!   band in this isolate; work the cursor has not passed `REDELIVER_AFTER_MS` later, and a failed first attempt, is
//!   sent again through the site's queue (`PRSYNC`, whose handler tells the two message kinds apart).
//! - **Per-base timer.** `step` answers `Idle { at }` when a check times out or `min_wait_minutes` ends with no new
//!   entry. The first attempt hands each base's `at` back, and the alarm runs that base again in band when it is due.
//!   A new leadership starts with every base that has a queue due at once, so a timer lost with the old leader is
//!   found again from the fold.
//! - **Consumer.** `one` reads each base's `QueueView` fresh from the leader (`{"q": {"mergequeue": base}}`), runs
//!   `step`, answers each `CacheAsk` on this site's git cache shard (`/merge` with `method`), commits the commands
//!   under their content's request ids, turns a refused landing into its `on_refused` follow-up, and reads again,
//!   until the base is idle. The batch's last command moves the cursor (`MergeCmd::Cursor`). A duplicate costs only a
//!   refusal or an answer from the `request` row.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use graftyard_git as git;
use graftyard_git::merge::MergeCmd;
use graftyard_git::merge_call::{MergeCall, MergeRefusal};
use graftyard_git::prsync::{Delivery, Dispatcher, SyncBatch, WAIT_MS};
use graftyard_git::queue_consumer::{CacheAnswer, CacheAsk, QueueRead, Step};
use graftyard_git::refs::RefName;
use graftyard_lookup::Bindings;
use graftyard_raft::{Index, Role};
use graftyard_types::{ChainId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::{SiteEnv, now_ms};

const FIRST_FOR: std::time::Duration = std::time::Duration::from_secs(50);
/// A base is planned at most this many rounds per attempt [guess]: a round is one read, its asks, and its commits.
const ROUNDS: usize = 8;
/// A base whose first attempt failed is run in band again this soon [guess], besides the queue's re-send.
const RETRY_MS: u64 = 30_000;
/// The earliest a timer fires after it is set, so a step whose `at` is already past does not spin.
const TIMER_MIN_MS: u64 = 1_000;
/// Test installs: a base whose landing was held plans again this long after, if the driver did not commit it.
#[cfg(feature = "test-faults")]
const HELD_WAKE_MS: u64 = 120_000;

type Leadership = (u64, u64);

#[derive(Default)]
struct Lead {
    leadership: Leadership,
    d: Dispatcher<RefName>,
    /// Each base's next due time.
    timers: BTreeMap<RefName, u64>,
    /// A first attempt runs.
    running: bool,
}

thread_local! {
    static LEADS: RefCell<HashMap<String, Lead>> = RefCell::default();
    /// Test installs: the leader's next landing is sent with this kill (`crate::faults::Kill`) and down time.
    #[cfg(feature = "test-faults")]
    static KILL: RefCell<HashMap<String, (String, u64)>> = RefCell::default();
    /// Test installs: the next `n` landings are not committed but kept here, for BF-1's shared entry.
    #[cfg(feature = "test-faults")]
    static HOLD: RefCell<HashMap<String, (u32, Vec<Value>)>> = RefCell::default();
}

fn key_of(h: &Rc<Host<Repo>>) -> String {
    h.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()).unwrap_or_default()
}

/// Test installs: `/bench_seed {"mqKill": {"kill": mode, "down_ms": n}}`, `{"mqHold": n}`, and `{"mqHeld": true}`.
#[cfg(feature = "test-faults")]
pub fn fault(h: &Rc<Host<Repo>>, body: &Value) -> Value {
    let key = key_of(h);
    if let Some(k) = body.get("mqKill") {
        let mode = k["kill"].as_str().unwrap_or("after_quorum").to_owned();
        let down = k["down_ms"].as_u64().unwrap_or(0);
        KILL.with(|m| m.borrow_mut().insert(key, (mode.clone(), down)));
        return json!({ "mqKill": mode, "down_ms": down });
    }
    if let Some(n) = body.get("mqHold").and_then(Value::as_u64) {
        HOLD.with(|m| m.borrow_mut().insert(key, (n as u32, Vec::new())));
        return json!({ "mqHold": n });
    }
    // Read, not taken: the hold stays until `{"mqHold": 0}`, so the consumer cannot land a held group meanwhile.
    let held = HOLD.with(|m| m.borrow().get(&key).map(|(_, v)| v.clone()).unwrap_or_default());
    json!({ "held": held })
}

/// One message on the site queue: a batch of the bases one repository's entries touched.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Marks the kind: the site queue carries `PrSync` messages too.
    pub merge_queue: u8,
    pub repo: i64,
    pub batch: SyncBatch<RefName>,
    pub record: graftyard_types::PlacementRecord,
    /// The member key of the leader that sent it, for the test-faults kill and hold.
    #[serde(default)]
    pub leader: String,
}

/// The leader's half, on the member's alarm, after `PrSync`'s.
pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    let leading = {
        let core = h.core.borrow();
        core.as_ref().filter(|c| c.m.role() == Role::Leader).and_then(|c| match c.join.chain {
            ChainId::Repo(repo) => {
                let fold = &c.m.group_fold().fold;
                let cursor = git::tiers::consumer_cursor(&*fold.core, git::issues::Consumer::MergeQueue);
                let bases = git::queue::queue_bases(&*fold.core);
                Some(((c.m.term().0, c.m.boot()), repo, cursor, c.m.state().applied, c.join.record.clone(), bases))
            }
            _ => None,
        })
    };
    let Some((leadership, repo, cursor, applied, record, bases)) = leading else { return };
    let key = key_of(h);
    let now = now_ms();
    let mut lead = LEADS.with(|m| m.borrow_mut().remove(&key)).filter(|l| l.leadership == leadership).unwrap_or_else(
        // A new leadership: every base with a queue is due now, so no timer is lost with the old leader.
        || Lead { leadership, timers: bases.into_iter().map(|b| (b, now)).collect(), ..Lead::default() },
    );
    if let Err(e) = scan(h, &mut lead.d, repo, cursor, applied) {
        note(h, format!("mergequeue scan: {e}"));
    }
    if h.fold_lost() {
        LEADS.with(|m| m.borrow_mut().insert(key, lead));
        return;
    }
    let mut first = None;
    match lead.d.batch(cursor, Timestamp(now)) {
        Some((b, Delivery::First)) => first = Some((b, true)),
        Some((b, Delivery::Again)) => {
            let m = Message { merge_queue: 1, repo: repo.0.0, batch: b, record: record.clone(), leader: key.clone() };
            if let Err(e) = enqueue(h, m).await {
                note(h, format!("mergequeue send: {e}"));
                lead.d.unsent();
            }
        }
        None => {}
    }
    // Due timers ride on the first attempt, or run alone when no batch is due and none runs.
    let due: Vec<RefName> = lead.timers.iter().filter(|(_, at)| **at <= now).map(|(b, _)| b.clone()).collect();
    if first.is_none() && !due.is_empty() && !lead.running {
        first = Some((SyncBatch { from: cursor, through: cursor, prs: Vec::new() }, false));
    }
    if let Some((b, _)) = first.as_mut() {
        for base in due {
            lead.timers.remove(&base);
            if !b.prs.contains(&base) {
                b.prs.push(base);
            }
        }
    }
    let waiting = lead.d.waiting(cursor);
    let next_timer = lead.timers.values().min().copied();
    if first.is_some() {
        lead.running = true;
    }
    LEADS.with(|m| m.borrow_mut().insert(key.clone(), lead));
    if let Some((batch, carried)) = first {
        let m = Message { merge_queue: 1, repo: repo.0.0, batch, record, leader: key.clone() };
        let w = Rc::downgrade(h);
        let env = h.env.clone();
        spawn_local(async move {
            let t0 = now_ms();
            let r = crate::host::within(FIRST_FOR, one(&env, &m))
                .await
                .unwrap_or_else(|| Err(format!("no answer in {} s", FIRST_FOR.as_secs())));
            let (from, through) = (m.batch.from.0, m.batch.through.0);
            match &r {
                Ok(v) => console_log!("mergequeue {} {from}..{through} first handled {}ms {v}", m.repo, now_ms() - t0),
                Err(e) => console_error!("mergequeue {} {from}..{through} first: {e}", m.repo),
            }
            LEADS.with(|l| {
                if let Some(lead) = l.borrow_mut().get_mut(&key)
                    && lead.leadership == leadership
                {
                    lead.running = false;
                    // Only a batch the dispatcher handed out is its attempt; a timer's is not.
                    if carried {
                        lead.d.finished(r.is_ok());
                    }
                    let now = now_ms();
                    match &r {
                        Ok(v) => {
                            for (b, at) in wakes(v) {
                                lead.timers.insert(b, at.max(now + TIMER_MIN_MS));
                            }
                        }
                        Err(_) => {
                            for b in &m.batch.prs {
                                lead.timers.entry(b.clone()).or_insert(now + RETRY_MS);
                            }
                        }
                    }
                }
            });
            let Some(h) = w.upgrade() else { return };
            h.arm_in(crate::prsync::LEADER_ALARM_MS);
        });
    }
    if waiting {
        h.arm_in(WAIT_MS);
    }
    if let Some(at) = next_timer {
        h.arm_in((at.saturating_sub(now) as i64).max(crate::prsync::LEADER_ALARM_MS));
    }
}

async fn enqueue(h: &Rc<Host<Repo>>, m: Message) -> Result<()> {
    h.env.queue("PRSYNC")?.send(m).await?;
    h.stats.borrow_mut().prsync_sent += 1;
    Ok(())
}

/// The timers a handled batch handed back: `{"wake": [[base, at]]}`.
fn wakes(v: &Value) -> Vec<(RefName, u64)> {
    serde_json::from_value(v["wake"].clone()).unwrap_or_default()
}

fn scan(h: &Rc<Host<Repo>>, d: &mut Dispatcher<RefName>, repo: RepoId, cursor: Index, applied: Index) -> Result<()> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| crate::host::err("not joined"))?;
    let fold = &c.m.group_fold().fold;
    let sql = h.state.storage().sql();
    let from = cursor.max(Index(fold.first_index().0.saturating_sub(1)));
    git::queue_consumer::scan(d, &fold.core, repo, from, applied, |i| {
        c.m.entry_at(i).cloned().or_else(|| crate::host::stored_entry::<Repo>(&sql, i))
    })
    .map_err(|i| crate::host::err(format!("entry {} is in neither the member nor the stored log", i.0)))
}

fn note(h: &Rc<Host<Repo>>, m: String) {
    console_error!("{m}");
    let mut s = h.stats.borrow_mut();
    if s.alarms.len() < 50 {
        s.alarms.push(format!("{} {m}", now_ms()));
    }
}

/// One batch, from either path: every base until idle, then the cursor. The answer names each base's timer.
pub async fn one(env: &Env, m: &Message) -> std::result::Result<Value, String> {
    let repo = RepoId(DbId(m.repo));
    let chain = ChainId::Repo(repo);
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = Bindings { env, sites: &site.sites };
    let mut wake = Vec::new();
    let mut done = Vec::new();
    let mut cursor = None;
    for base in &m.batch.prs {
        let (c, at, cmds) = run_base(env, &site, &b, m, chain, base).await?;
        cursor = Some(c);
        done.push(json!([base.display(), cmds]));
        if let Some(at) = at {
            wake.push((base.clone(), at.0));
        }
    }
    // A batch that touched no base still moves the cursor past its entries, once it is ahead of the committed one.
    if m.batch.through > m.batch.from && cursor.is_none_or(|c| m.batch.through > c) {
        let cmd = MergeCmd::Cursor(m.batch.through);
        commit(&b, chain, &m.record, cursor_rid(repo, m.batch.through), &cmd, None)
            .await?
            .map_err(|r| format!("{r:?}"))?;
    }
    Ok(json!({ "bases": done, "wake": wake }))
}

/// The cursor command's request id, from what it covers: a redelivered batch's is the same. The top 16 bits are a
/// tag no edge-minted id has, as `PrSync`'s and the queue commands' are.
fn cursor_rid(repo: RepoId, through: Index) -> graftyard_raft::RequestId {
    graftyard_raft::RequestId(graftyard_types::Ulid(
        (0x4d43u128 << 112) | (u128::from(repo.0.0 as u64 & 0xffff_ffff_ffff) << 64) | u128::from(through.0),
    ))
}

/// Plans and commits on one base until `step` is idle. The cursor read, the idle time, and the commands committed.
async fn run_base(
    env: &Env,
    site: &SiteEnv,
    b: &Bindings<'_>,
    m: &Message,
    chain: ChainId,
    base: &RefName,
) -> std::result::Result<(Index, Option<Timestamp>, usize), String> {
    let mut committed = 0;
    let mut cursor = Index(0);
    'rounds: for _ in 0..ROUNDS {
        let q = json!({ "q": { "mergequeue": base } });
        let (v, _) = graftyard_lookup::call(b, chain, &m.record, "/read", &q).await?;
        let read: QueueRead =
            serde_json::from_value(v["value"].clone()).map_err(|e| format!("mergequeue read answered {v}: {e}"))?;
        cursor = read.cursor;
        let view = read.view;
        let mut answers: Vec<CacheAnswer> = Vec::new();
        let cmds = loop {
            match git::queue_consumer::step(&view, Timestamp(now_ms()), &answers) {
                Step::Idle { at } => return Ok((cursor, at, committed)),
                Step::Ask(ask) => {
                    if answers.len() >= 4 {
                        return Err("step kept asking".into());
                    }
                    answers.push(cache(env, site, m.repo, &ask).await?);
                }
                Step::Submit(cmds) => break cmds,
            }
        };
        for cmd in cmds {
            let rid = git::queue_consumer::request_id(&cmd, &view);
            #[cfg(feature = "test-faults")]
            if held(&cmd, rid, &m.leader) {
                // Idle until the wake, so neither this attempt's failure nor a re-send lands it meanwhile.
                return Ok((cursor, Some(Timestamp(now_ms() + HELD_WAKE_MS)), committed));
            }
            committed += 1;
            if let Err(refusal) = commit(b, chain, &m.record, rid, &cmd, Some(&m.leader)).await? {
                console_log!("mergequeue {} refused #{}: {:?}", base.display(), refusal.n.0, refusal.err);
                if let Some(f) = git::queue_consumer::on_refused(&cmd, refusal.n, &refusal.err) {
                    let rid = git::queue_consumer::request_id(&f, &view);
                    let _ = commit(b, chain, &m.record, rid, &f, None).await?;
                }
                // A refusal means the view changed under the plan: read again.
                continue 'rounds;
            }
        }
    }
    Ok((cursor, None, committed))
}

/// Test installs: a landing `mqHold` holds is kept for `{"mqHeld": true}` instead of committed, and so is every
/// landing after it until `{"mqHold": 0}`.
#[cfg(feature = "test-faults")]
fn held(cmd: &MergeCmd, rid: graftyard_raft::RequestId, key: &str) -> bool {
    if !matches!(cmd, MergeCmd::Land(_)) {
        return false;
    }
    let call = MergeCall { cmd: cmd.clone(), render: Value::Null };
    let held = HOLD.with(|m| {
        let mut m = m.borrow_mut();
        let (n, list) = m.get_mut(key)?;
        if *n == 0 {
            // Holding already: a later plan of the same landing waits too, until the driver releases the hold.
            return (!list.is_empty()).then_some(());
        }
        *n -= 1;
        list.push(json!({ "rid": rid.to_string(), "cmd": git::repo::RepoCmd::Merge(Box::new(call)) }));
        Some(())
    });
    if held.is_some() {
        console_log!("mergequeue landing held (test fault)");
    }
    held.is_some()
}

/// Commits one command; `Ok(Err(refusal))` when `decide` refused it.
async fn commit(
    b: &Bindings<'_>,
    chain: ChainId,
    record: &graftyard_types::PlacementRecord,
    rid: graftyard_raft::RequestId,
    cmd: &MergeCmd,
    leader: Option<&str>,
) -> std::result::Result<std::result::Result<(), MergeRefusal>, String> {
    let call = MergeCall { cmd: cmd.clone(), render: Value::Null };
    let body = serde_json::to_value(git::repo::RepoCmd::Merge(Box::new(call))).map_err(|e| e.to_string())?;
    #[cfg(feature = "test-faults")]
    if let (MergeCmd::Land(_), Some(key)) = (cmd, leader) {
        if let Some((kill, down)) = KILL.with(|m| m.borrow_mut().remove(key)) {
            console_log!("mergequeue landing sent with kill {kill} (test fault)");
            let r = graftyard_lookup::call(
                b,
                chain,
                record,
                "/commit",
                &json!({ "rid": rid.to_string(), "cmd": body, "kill": kill, "down_ms": down }),
            )
            .await;
            console_log!("mergequeue killed landing answered {r:?}");
        }
    }
    #[cfg(not(feature = "test-faults"))]
    let _ = leader;
    let done = graftyard_lookup::commit_as(b, chain, record, rid, body).await?;
    if done.get("ok").is_some() {
        return Ok(Ok(()));
    }
    match done.pointer("/refused/merge/refusal").cloned().map(serde_json::from_value::<MergeRefusal>) {
        Some(Ok(r)) => Ok(Err(r)),
        _ => Err(format!("commit: {done}")),
    }
}

/// A `CacheAsk` on this site's shard of the network, as `crate::prsync` asks for a test merge.
async fn cache(env: &Env, site: &SiteEnv, repo: i64, ask: &CacheAsk) -> std::result::Result<CacheAnswer, String> {
    let me = site.id;
    let region = site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", me.0))
        .and_then(|ns| ns.get_by_name_with_location_hint(&format!("git#{}#net:{repo}", me.0), region.hint()))
        .map_err(|e| e.to_string())?;
    let (on, heads, method) = git::merge_cache::chain_of(ask);
    let url = format!("https://shard/merge?repo={repo}&{}", git::merge_cache::chain_query(on, &heads, method));
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("shard merge: {} {text}", r.status_code()));
    }
    let a: git::merge_cache::ChainAnswer =
        serde_json::from_str(&text).map_err(|e| format!("shard merge answered {text}: {e}"))?;
    a.answer(ask).map_err(|e| format!("{e:?}"))
}

/// The leader's answer to the consumer's read: the cursor and the base's `QueueView`.
pub fn read_json(fold: &git::repo::RepoFold, base: &Value) -> Value {
    match serde_json::from_value::<RefName>(base.clone()) {
        Ok(b) => json!(QueueRead::of(&fold.core, &b)),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

/// Test installs: records events on the leader as `crate::prsync::open_pr` does, for what no client route sets yet:
/// `{"record": [RepoEvent..]}` (a branch rule and the repository settings for the gates). The fold checks each.
#[cfg(feature = "test-faults")]
pub async fn record(h: &Rc<Host<Repo>>, events: &Value) -> Result<Value> {
    use graftyard_raft::{Fold as _, Scratch as _};
    let events: Vec<git::repo::RepoEvent> =
        serde_json::from_value(events.clone()).map_err(|e| Error::RustError(e.to_string()))?;
    {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| crate::host::err("not joined"))?;
        let mut s = c.m.group_fold().fold.scratch();
        for ev in &events {
            if let Err(why) = s.check(ev) {
                return Ok(json!({ "refused": why.to_string() }));
            }
            s.apply(ev);
        }
    }
    let rid = graftyard_raft::RequestId(graftyard_types::Ulid(
        (0x5244u128 << 112) | u128::from(now_ms()) << 16 | u128::from(js_sys::Math::random().to_bits() as u16),
    ));
    let rx = h.wait_recorded(rid);
    if let Err(e) = h.input(|m, t| m.record(t, rid, git::repo::REPO_FORMAT_V1, events))? {
        return Ok(json!({ "error": crate::host::call_error_json(&e) }));
    }
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(index))) => Ok(json!({ "recorded": index.0 })),
        Some(Ok(Err(nl))) => Ok(json!({ "error": crate::host::nl_json(&nl) })),
        _ => Ok(json!({ "busy": true })),
    }
}
