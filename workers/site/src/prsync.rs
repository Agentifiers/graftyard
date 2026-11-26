//! The `PrSync` consumer on a site script (`design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, "Dominant access
//! patterns", Webhook: leader alarm, cursor, Queue message, consumer Worker). The pure half is
//! `graftyard_git::prsync`.
//!
//! - **Leader alarm.** A repository member that leads runs its `Dispatcher` on the alarm after it applies entries. It
//!   reads the log from the group's committed `PrSync` cursor. Each batch's first attempt runs at once, without a queue
//!   hop: the alarm sends it, in the background, to a new invocation of this site script through its own `SITE`
//!   binding (`in_band`), which runs the consumer's handler (`one`), so the alarm, the member's Raft inputs, and its
//!   replies never wait on the git cache, and the handler's calls are not charged to the member's requests. One first attempt runs at a time per member; batches meanwhile wait
//!   and go out as one. Work the cursor has not passed `REDELIVER_AFTER_MS` after it was handed out is sent again
//!   through the site's queue (`PRSYNC`), as is a first attempt that failed. The dispatcher lives in this isolate's
//!   memory for one leadership, so a reset member or a new leader starts again at the cursor.
//! - **Consumer.** `one` is the handler for both paths. It reads each batch's pull requests fresh from the group's
//!   leader, has the network's git cache shard on its own site compute every missing test merge (`/merge`), and
//!   commits the batch's `PrSync` commands under their deterministic request ids. The queue handler acks a message
//!   only once its last command, which moves the cursor, committed; anything else is retried by Queues. Neither path
//!   is needed for correctness: a duplicate commits nothing, and work no attempt finished is covered by the cursor and
//!   the re-send. No path calls the Cloudflare API: the first attempt uses the script's own service binding, the
//!   re-send the queue binding.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_git as git;
use graftyard_git::prsync::{ALARM_MS, Delivery, Dispatcher, SyncBatch, WAIT_MS};
use graftyard_lookup::Bindings;
use graftyard_raft::{Index, Role};
use graftyard_types::{ChainId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::MessageExt;
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::{SiteEnv, now_ms};

/// How soon after it applies entries a leading repository member's alarm runs the dispatcher.
pub const LEADER_ALARM_MS: i64 = ALARM_MS;

/// An attempt's run, first or re-sent (`git::prsync::FIRST_FOR_MS`): the invocation that runs it ends it there.
const FIRST_FOR: std::time::Duration = std::time::Duration::from_millis(git::prsync::FIRST_FOR_MS);
/// How much longer than its attempt's deadline the leader waits for the answer [guess: a same-site service binding's
/// answer, and the two isolates' clocks]. Past it the attempt counts as failed and its batch goes to the queue at
/// once, and by then the attempt has ended, so the leader never starts a second while the first runs. Under
/// REDELIVER_AFTER_MS - FIRST_FOR_MS, so the dispatcher hears the answer before it would count the attempt as lost
/// (`Dispatcher::batch`).
const ANSWER_MARGIN_MS: u64 = 2_000;
const _: () = assert!(git::prsync::FIRST_FOR_MS + ANSWER_MARGIN_MS < git::prsync::REDELIVER_AFTER_MS);

/// Which leadership a dispatcher belongs to: the term, and the member's boot, so an object reset and restored in the
/// same isolate and term (a `/reset`, a failed write) starts a new one and waits on no attempt of its old self.
type Leadership = (u64, u64);

thread_local! {
    /// Per member object of this isolate: the leadership its dispatcher belongs to, and the dispatcher.
    static DISPATCHERS: RefCell<HashMap<String, (Leadership, Dispatcher)>> = RefCell::default();
    /// Per member object of this isolate: when its leadership last caught up its network's shards, and the applied
    /// index then (`graftyard_git::keep_warm_due`).
    static WARMED: RefCell<HashMap<String, (Leadership, u64, Index)>> = RefCell::default();
    /// Test installs: first attempts to drop, per member object, as if each was lost with its isolate.
    #[cfg(feature = "test-faults")]
    static DROP_FIRST: RefCell<HashMap<String, u32>> = RefCell::default();
    /// Test installs: a shorter FIRST_FOR, per member object (`first_for`).
    #[cfg(feature = "test-faults")]
    static FIRST_FOR_MS: RefCell<HashMap<String, u64>> = RefCell::default();
}

/// Test installs: this member gives each first attempt `ms` before it ends and counts as failed, and is sent again
/// through the queue (`/bench_seed {"prsyncFirstFor": ms}`, 0 to clear). The attempt ends at that deadline too, so a
/// short one shows an attempt cut halfway, then its re-send. Before LM-1 round 16d the attempt ran on, so its merges
/// and the re-send's reached the shard at once (`tests/am1/results/merge-budget-3.md`).
#[cfg(feature = "test-faults")]
pub fn first_for(h: &Rc<Host<Repo>>, ms: u64) -> Value {
    let key = h.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()).unwrap_or_default();
    FIRST_FOR_MS.with(|d| if ms == 0 { d.borrow_mut().remove(&key) } else { d.borrow_mut().insert(key, ms) });
    json!({ "prsyncFirstFor": ms })
}

/// Test installs: this member drops its next `n` first attempts unrun (`/bench_seed {"prsyncDrop": n}`), so only the
/// cursor and the re-send can cover their batches.
#[cfg(feature = "test-faults")]
pub fn drop_first(h: &Rc<Host<Repo>>, n: u32) -> Value {
    let key = h.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()).unwrap_or_default();
    DROP_FIRST.with(|d| d.borrow_mut().insert(key, n));
    json!({ "prsyncDrop": n })
}

/// One queue message: a batch of one repository chain, and the chain's placement record as its leader joined with,
/// so the consumer skips the two `Names` and `Authz` reads (about 1 s on the test install). A message from before
/// the record rode along has none, and the consumer reads it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub repo: i64,
    pub batch: SyncBatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<graftyard_types::PlacementRecord>,
    /// A first attempt's end, in ms on the leader's clock. Its invocation ends it then, or FIRST_FOR after it started
    /// it if that comes first (`attempt`), so an invocation that starts late ends no later than the leader expects,
    /// and one whose clock runs behind still ends within FIRST_FOR. The leader waits for its answer until then and
    /// ANSWER_MARGIN_MS. A queue message has none, so its attempt runs FIRST_FOR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
}

/// The leader's half, on the member's alarm.
pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    let leading = {
        let core = h.core.borrow();
        core.as_ref().filter(|c| c.m.role() == Role::Leader).and_then(|c| match c.join.chain {
            ChainId::Repo(repo) => {
                let fold = &c.m.group_fold().fold;
                let cursor = git::tiers::consumer_cursor(&*fold.core, git::issues::Consumer::PrSync);
                Some((
                    c.join.me.do_name.to_string(),
                    (c.m.term().0, c.m.boot()),
                    repo,
                    cursor,
                    c.m.state().applied,
                    c.join.record.clone(),
                ))
            }
            _ => None,
        })
    };
    let Some((key, term, repo, cursor, applied, record)) = leading else { return };
    let (mut d, new) = DISPATCHERS.with(|m| match m.borrow().get(&key) {
        Some((t, d)) if *t == term => (d.clone(), false),
        _ => (Dispatcher::default(), true),
    });
    if new {
        // A chain this member parked stays parked under its new leadership, so a stuck batch costs each member at
        // most PARK_AFTER attempts, and the shard wake-ups they cost, however often the group re-elects.
        if h.state.storage().get::<u64>(PARK_KEY).await.ok().flatten() == Some(cursor.0) {
            d.park(cursor);
        }
    }
    let last = WARMED.with(|m| m.borrow().get(&key).filter(|(t, _, _)| *t == term).map(|(_, at, i)| (*at, *i)));
    if git::keep_warm_due(last, now_ms(), applied) {
        WARMED.with(|m| m.borrow_mut().insert(key.clone(), (term, now_ms(), applied)));
        warm(h, repo, &record);
    }
    let was_parked = d.parked(cursor);
    // A scan cut at `SCAN_BYTES` comes back at once for the rest.
    match scan(h, &mut d, repo, cursor, applied) {
        Ok(true) => h.arm_in(LEADER_ALARM_MS),
        Ok(false) => {}
        Err(e) => note(h, format!("prsync scan: {e}")),
    }
    // A scan that lost a fold read may have missed a pull request an entry touched; a batch past it would skip it.
    if h.fold_lost() {
        return;
    }
    let batch = d.batch(cursor, Timestamp(now_ms()));
    let mut first = None;
    match batch {
        Some((b, Delivery::First)) => {
            first = Some(Message { repo: repo.0.0, batch: b, record: Some(record), deadline: None })
        }
        Some((b, Delivery::Again)) => {
            let m = Message { repo: repo.0.0, batch: b, record: Some(record), deadline: None };
            if let Err(e) = enqueue(h, &m).await {
                d.unsent();
                note(h, format!("prsync send {}..{}: {e}", m.batch.from.0, m.batch.through.0));
            }
        }
        None => {}
    }
    // Parked: `design/SHARED.md`, "Event fan-out", Isolation. The park is the chain's SLO signal: an error in the
    // logs, `prsyncParked` in `/stats`, and the member's stored cursor, which `/prsync {"replay": true}` clears.
    if d.parked(cursor) {
        h.stats.borrow_mut().prsync_parked = cursor.0;
        if !was_parked {
            note(h, format!("prsync {} parked at {} after {} batches", repo.0.0, cursor.0, git::prsync::PARK_AFTER));
            if let Err(e) = h.state.storage().put(PARK_KEY, cursor.0).await {
                note(h, format!("prsync park: {e}"));
            }
        }
    }
    let waiting = d.waiting(cursor);
    DISPATCHERS.with(|m| m.borrow_mut().insert(key.clone(), (term, d)));
    #[cfg(feature = "test-faults")]
    if first.is_some()
        && DROP_FIRST.with(|d| d.borrow_mut().get_mut(&key).filter(|n| **n > 0).map(|n| *n -= 1).is_some())
    {
        console_log!("prsync {} first dropped (test fault)", repo.0.0);
        first = None;
    }
    if let Some(mut m) = first {
        h.stats.borrow_mut().prsync_first += 1;
        // Weak, as `Host::kick`'s loop: a member reset while its attempt runs must not stay alive for it (F47).
        let w = Rc::downgrade(h);
        let env = h.env.clone();
        #[allow(unused_mut)]
        let mut first_for = FIRST_FOR;
        #[cfg(feature = "test-faults")]
        if let Some(ms) = FIRST_FOR_MS.with(|d| d.borrow().get(&key).copied()) {
            // Never longer than FIRST_FOR, which `attempt` holds it to anyway: a longer wait would outlast
            // REDELIVER_AFTER_MS, and the dispatcher would start a second first attempt while this one ran.
            first_for = std::time::Duration::from_millis(ms).min(FIRST_FOR);
        }
        let t0 = now_ms();
        m.deadline = Some(t0 + first_for.as_millis() as u64);
        spawn_local(async move {
            // The invocation `in_band` starts ends the attempt by its deadline (`attempt`), so this member waits for
            // its answer. Only past the deadline and ANSWER_MARGIN_MS, when the attempt has ended, does it count it as
            // failed and let the dispatcher hand out the next first attempt.
            let wait = first_for + std::time::Duration::from_millis(ANSWER_MARGIN_MS);
            let r = crate::host::within(wait, in_band(&env, &m))
                .await
                .unwrap_or_else(|| Err(format!("no answer in {} ms", wait.as_millis())));
            let ms = now_ms() - t0;
            let (from, through) = (m.batch.from.0, m.batch.through.0);
            match &r {
                Ok(v) => console_log!("prsync {} {from}..{through} first handled {ms}ms {v}", m.repo),
                Err(e) => console_error!("prsync {} {from}..{through} first: {e}", m.repo),
            }
            // A failed attempt is sent again through the queue at once, not after `REDELIVER_AFTER_MS`.
            DISPATCHERS.with(|m| {
                if let Some((t, d)) = m.borrow_mut().get_mut(&key)
                    && *t == term
                {
                    d.finished(r.is_ok());
                }
            });
            let Some(h) = w.upgrade() else { return };
            if let Ok(v) = &r {
                h.stats.borrow_mut().test_merges(v);
            }
            // Batches that waited for this attempt, or the re-send of a failed one.
            h.arm_in(LEADER_ALARM_MS);
        });
    }
    if waiting {
        h.arm_in(WAIT_MS);
    }
}

/// The path of a first attempt on the site script's own fetch handler (`in_band`).
pub const FIRST_PATH: &str = "/prsync/first";

/// Runs a first attempt in a new invocation of this site script, through its own service binding (`SITE`), not in the
/// leader's: the Workers runtime charges every call an object makes to the newest request the object received
/// (`tests/m3/results/m3-wp5.md`, "Where the depth comes from"), so one attempt's `/merge` calls, each answered after the
/// shard's own reads of this member, left one hop deeper per merge and failed "Subrequest depth limit exceeded" at its
/// eighth (`tests/am1/results/merge-budget-3.md`). A stateless invocation starts its calls at depth 1 and receives
/// none. Its handler is `one`, as the queue's is.
async fn in_band(env: &Env, m: &Message) -> std::result::Result<Value, String> {
    let body = serde_json::to_string(m).map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post).with_body(Some(body.into()));
    let req = Request::new_with_init(&format!("https://site{FIRST_PATH}"), &init).map_err(|e| e.to_string())?;
    let mut r = env.service("SITE").map_err(|e| e.to_string())?.fetch_request(req).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    match r.status_code() {
        200 => serde_json::from_str(&text).map_err(|e| format!("first attempt answered {text}: {e}")),
        s => Err(format!("first attempt: {s} {text}")),
    }
}

/// The site script's fetch handler for `in_band`: one batch, answered with what `attempt` returned. A 500 carries its
/// error.
pub async fn first_handled(mut req: Request, env: Env, ctx: &Context) -> Result<Response> {
    let m: Message = req.json().await?;
    // The attempt runs under `wait_until`, so a dropped connection does not cancel it halfway; it ends at its deadline
    // either way (`attempt`), and its commands are idempotent.
    let (tx, rx) = futures_channel::oneshot::channel();
    ctx.wait_until(async move {
        let _ = tx.send(attempt(&env, &m).await);
    });
    match rx.await {
        Ok(Ok(v)) => Response::from_json(&v),
        Ok(Err(e)) => Response::error(e, 500),
        Err(_) => Response::error("first attempt dropped", 500),
    }
}

/// The member's stored cursor at which it parked the chain for `PrSync`.
const PARK_KEY: &str = "prsync_park";

/// The operator's replay of a parked chain (`design/SHARED.md`, "Event fan-out", Isolation): the member forgets its
/// park and its dispatcher, and its next alarm, as a new leadership, sends the work from the cursor again.
pub async fn replay(h: &Rc<Host<Repo>>) -> Result<Value> {
    let key = h.core.borrow().as_ref().map(|c| c.join.me.do_name.to_string()).unwrap_or_default();
    let was = h.state.storage().get::<u64>(PARK_KEY).await.ok().flatten();
    h.state.storage().delete(PARK_KEY).await?;
    DISPATCHERS.with(|m| m.borrow_mut().remove(&key));
    h.stats.borrow_mut().prsync_parked = 0;
    h.arm_in(LEADER_ALARM_MS);
    Ok(json!({ "replayed": was }))
}

/// A leadership catches up every Full site's first shard of the network, if it runs: at once, then every
/// `KEEP_WARM_MS` while the group applies entries (`graftyard_git::keep_warm_due`). Merges and test merges run on the
/// leader's site's shard (`merge`), and one that last served under another leader is behind every push validated and
/// merged elsewhere since, so without this the first merge after an election caught up first: 270 to 540 ms of every
/// merge on a freshly elected leader, in 91 of 91 and 29 of 29 samples (`tests/m2/results/m2-wp6.md`, BM-2). The other
/// sites' shards are where pushes go when the home's is killed, and caught up only by those pushes they were 1,100 and
/// 1,900 entries behind, so every push in flight at j9g's kill waited out both and answered 503
/// (`tests/am1/results/hot-shards-3.md`). Best effort: a failed catch-up leaves the call's own to run. The call carries
/// `warm`, so a stopped shard answers without starting its container (`graftyard_git::serve`): a group with no clients
/// re-elects after its members idle out, and each warm started its network's shard, which then billed for a day
/// (`tests/am1/results/idle-shards.md`). Each call is one background task, not a loop: the home shard's catch-up
/// reads this member back, but a repeat comes only after entries applied, so writes, each a new shallow request here,
/// came in between and reset the depth its next call is charged to (`tests/m3/results/m3-wp5.md`, point 6).
fn warm(h: &Rc<Host<Repo>>, repo: RepoId, record: &graftyard_types::PlacementRecord) {
    let Ok(rec) = serde_json::to_string(record) else { return };
    for at in git::promote_sites(record, record.home) {
        let env = h.env.clone();
        let rec = rec.clone();
        spawn_local(async move {
            let t0 = now_ms();
            let r = async {
                let site = SiteEnv::read(&env).map_err(|e| e.to_string())?;
                let region = site.sites.get(at).ok_or("a site is not in GRAFTYARD_SITES")?.region;
                let stub = env
                    .durable_object(&format!("SHARD_{}", at.0))
                    .and_then(|ns| {
                        ns.get_by_name_with_location_hint(
                            &git::first_shard_name(graftyard_types::NetworkId::of_root(repo), at),
                            region.hint(),
                        )
                    })
                    .map_err(|e| e.to_string())?;
                let url = format!(
                    "https://shard/catchup?warm=1&repo={}&record={}",
                    repo.0.0,
                    js_sys::encode_uri_component(&rec)
                );
                let mut init = RequestInit::new();
                init.with_method(Method::Post);
                let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
                let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
                match r.status_code() {
                    // A stopped shard skips a warm (`graftyard_git::serve`): logged as one, not as a warm shard, which
                    // on B0-33 m61 hid that sites b and c were stopped until load started them
                    // (`tests/am1/results/b033-m61-why.md`).
                    200 => Ok(r.headers().get("x-graft-skipped").ok().flatten()),
                    s => Err(format!("{s} {}", r.text().await.unwrap_or_default())),
                }
            };
            match crate::host::within(FIRST_FOR, r).await {
                Some(Ok(Some(why))) => console_log!("shard warm {} site {} skipped: {why}", repo.0.0, at.0),
                Some(Ok(None)) => console_log!("shard warm {} site {} {}ms", repo.0.0, at.0, now_ms() - t0),
                Some(Err(e)) => console_error!("shard warm {} site {}: {e}", repo.0.0, at.0),
                None => console_error!("shard warm {} site {}: no answer in {} s", repo.0.0, at.0, FIRST_FOR.as_secs()),
            }
        });
    }
}

/// The re-send path: one message on this site's queue.
async fn enqueue(h: &Rc<Host<Repo>>, m: &Message) -> Result<()> {
    h.env.queue("PRSYNC")?.send(m.clone()).await?;
    h.stats.borrow_mut().prsync_sent += 1;
    Ok(())
}

fn scan(h: &Rc<Host<Repo>>, d: &mut Dispatcher, repo: RepoId, cursor: Index, applied: Index) -> Result<bool> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| crate::host::err("not joined"))?;
    let fold = &c.m.group_fold().fold;
    let sql = h.state.storage().sql();
    // Entries archived and truncated before this release ran its first `PrSync` are not read: they predate the
    // consumer (`tests/m2/results/m2-wp5.md`, "Design gaps"). Entries in memory are read from the member, older ones
    // from the stored log.
    let from = cursor.max(Index(fold.first_index().0.saturating_sub(1)));
    let start = from.max(d.scanned());
    let to = applied.0.min(start.0 + git::prsync::SCAN_MAX);
    if applied > start {
        crate::alarm_steps::note_scan(
            &sql,
            c.join.me.do_name.as_ref(),
            "prsync",
            start.0,
            to,
            Some(c.m.log_base().index.0),
        );
    }
    git::prsync::scan(d, &fold.core, repo, from, applied, |i| {
        c.m.entry_at(i).map(|e| (e.clone(), 0)).or_else(|| crate::host::stored_entry_sized::<Repo>(&sql, i))
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

/// The consumer Worker: each message is one batch, acked once its commands committed and retried otherwise.
pub async fn consume(batch: MessageBatch<Value>, env: Env) -> Result<()> {
    for m in batch.iter() {
        let m = match m {
            Ok(m) => m,
            Err(e) => {
                console_error!("prsync: a message that does not decode: {e}");
                continue;
            }
        };
        if m.body().get("exportFeed").is_some() {
            crate::export_feed::kick(&env).await;
            m.ack();
            continue;
        }
        if m.body().get("mergeQueue").is_some() {
            let q: crate::merge_queue::Message = match serde_json::from_value(m.body().clone()) {
                Ok(q) => q,
                Err(e) => {
                    console_error!("mergequeue: a message that does not decode: {e}");
                    m.ack();
                    continue;
                }
            };
            match crate::merge_queue::one(&env, &q).await {
                Ok(v) => {
                    console_log!("mergequeue {} {}..{} queued handled {v}", q.repo, q.batch.from.0, q.batch.through.0);
                    m.ack();
                }
                // Acked: the leader's dispatcher re-sends from the cursor and parks after PARK_AFTER, as for `PrSync`.
                Err(e) => {
                    console_error!("mergequeue {} {}..{}: {e}", q.repo, q.batch.from.0, q.batch.through.0);
                    m.ack();
                }
            }
            continue;
        }
        let body: Message = match serde_json::from_value(m.body().clone()) {
            Ok(b) => b,
            Err(e) => {
                console_error!("prsync: a message that does not decode: {e}");
                m.ack();
                continue;
            }
        };
        let t0 = now_ms();
        // From the leader's send to this handler, on two clocks.
        let queued = t0.saturating_sub(m.timestamp().as_millis());
        match attempt(&env, &body).await {
            Ok(v) => {
                console_log!(
                    "prsync {} {}..{} queued {queued}ms handled {}ms {v}",
                    body.repo,
                    body.batch.from.0,
                    body.batch.through.0,
                    now_ms() - t0
                );
                m.ack();
            }
            // Acked, not retried: the leader's dispatcher sends the work again from the cursor, and counts each
            // batch toward PARK_AFTER. Queues' own 10 retries a message multiplied every re-send by up to 11 test
            // merges, each one waking the network's shard (`tests/am1/results/idle-shards.md`).
            Err(e) => {
                console_error!("prsync {} {}..{}: {e}", body.repo, body.batch.from.0, body.batch.through.0);
                m.ack();
            }
        }
    }
    Ok(())
}

/// Each repository's owner and the owner's record, as the consumer last read them [guess: 60 s]. A record that
/// moved fails the next call, which reads it again.
const RECORD_FOR_MS: u64 = 60_000;

thread_local! {
    static RECORDS: RefCell<HashMap<i64, (u64, graftyard_types::PlacementRecord)>> = RefCell::default();
}

async fn record_of(env: &Env, repo: RepoId) -> std::result::Result<graftyard_types::PlacementRecord, String> {
    if let Some(r) =
        RECORDS.with(|m| m.borrow().get(&repo.0.0).filter(|(at, _)| now_ms() < at + RECORD_FOR_MS).cloned())
    {
        return Ok(r.1);
    }
    let owner = crate::member::repo_owner(env, repo).await?;
    let record = crate::member::owner_record(env, owner).await?;
    RECORDS.with(|m| m.borrow_mut().insert(repo.0.0, (now_ms(), record.clone())));
    Ok(record)
}

/// One attempt at a batch, first or re-sent: `one`, ended at its deadline, never more than FIRST_FOR from now.
/// Dropping `one` there drops its reads, merge answers, and calls, so an attempt the leader gave up on holds nothing
/// after it. `one` itself stops asking merges early enough to commit by then (`git::prsync::test_merges`), so a cut is
/// for a read or a commit that did not answer, and the batch is sent again from the cursor.
async fn attempt(env: &Env, m: &Message) -> std::result::Result<Value, String> {
    let start = now_ms();
    let end = m.deadline.unwrap_or(u64::MAX).min(start + git::prsync::FIRST_FOR_MS).max(start);
    crate::host::within(std::time::Duration::from_millis(end - start), one(env, m, start, end))
        .await
        .unwrap_or_else(|| Err(format!("attempt ended at its deadline, {} ms", end - start)))
}

async fn one(env: &Env, m: &Message, start: u64, end: u64) -> std::result::Result<Value, String> {
    let repo = RepoId(DbId(m.repo));
    let chain = ChainId::Repo(repo);
    let mut t = vec![now_ms()];
    let record = match m.record.clone() {
        Some(r) => r,
        None => record_of(env, repo).await.inspect_err(|_| {
            RECORDS.with(|m| m.borrow_mut().remove(&repo.0.0));
        })?,
    };
    t.push(now_ms());
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = Bindings { env, sites: &site.sites };
    let (v, _) =
        graftyard_lookup::call(&b, chain, &record, "/read", &json!({ "q": { "prsync": m.batch.prs } })).await?;
    let git::prsync::SyncRead { cursor, views, .. } =
        serde_json::from_value(v["value"].clone()).map_err(|e| format!("prsync read answered {v}: {e}"))?;
    t.push(now_ms());
    if cursor >= m.batch.through {
        return Ok(json!({ "skipped": cursor.0 }));
    }
    // TEST_MERGE_BUDGET is the shard's: a head push's merge comes back `Shed` past the budget by more than one merge, or
    // when its turn came past TURN_WAIT_MS, and its `mergeable` stays null. One the shard has not answered by the time
    // merges must stop counts as shed too, and so do those left unasked then.
    let budget = RefCell::new(Value::Null);
    let until = git::prsync::merges_until(start, end);
    let site = &site;
    let got = git::prsync::test_merges(&views, now_ms, until, |n, base, head, left| {
        let budget = &budget;
        async move {
            let wait = std::time::Duration::from_millis(left);
            match crate::host::within(wait, merge(env, site, repo, n, base, head)).await {
                Some(r) => {
                    let (a, b) = r?;
                    *budget.borrow_mut() = b;
                    Ok(a)
                }
                None => Ok(git::prsync::MergeAnswer::Shed { shed: true }),
            }
        }
    })
    .await?;
    t.push(now_ms());
    let (asked, unasked, shed_n) = (got.asked, got.unasked, got.shed.len());
    let budget = budget.into_inner();
    let cmds = git::prsync::commands(&m.batch, &views, got.merges, got.shed);
    let mut events = 0;
    for c in cmds {
        let rid = c.request;
        let cmd = serde_json::to_value(git::repo::RepoCmd::Sync(c)).map_err(|e| e.to_string())?;
        let done = graftyard_lookup::commit_as(&b, chain, &record, rid, cmd).await?;
        events +=
            done.pointer("/ok/out/Sync/events").and_then(Value::as_u64).ok_or_else(|| format!("commit: {done}"))?;
    }
    t.push(now_ms());
    // Milliseconds in each step: record, read, merges, commits.
    let steps: Vec<u64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    Ok(json!({
        "prs": views.len(),
        "events": events,
        "ms": steps,
        "asked": asked,
        "unasked": unasked,
        "shed": shed_n,
        "budget": budget
    }))
}

/// The test merge on this site's shard of the network, as a head push's (`MergeCause::HeadPush`), with the network's
/// budget counts the shard sent (`x-graft-budget`, `graftyard_git::prsync::BudgetStats`). A site script binds only
/// its own `GitShard` class (`workers/site/deploy.mjs`), and the queue is the leader's site's, so the merge runs
/// where the leader is; a shard that does not hold the network yet rehydrates it from the log and the buckets, as
/// any shard does.
async fn merge(
    env: &Env,
    site: &SiteEnv,
    repo: RepoId,
    n: git::issues::IssueNumber,
    base: git::refs::ObjectId,
    head: git::refs::ObjectId,
) -> std::result::Result<(git::prsync::MergeAnswer, Value), String> {
    let url = format!(
        "https://shard/merge?repo={}&base={}&head={}&n={}&cause={}",
        repo.0.0,
        base.hex(),
        head.hex(),
        n.0,
        git::prsync::MergeCause::HeadPush.as_str()
    );
    let (v, budget) = shard_post(env, site, repo, &url).await?;
    let a = serde_json::from_value(v.clone()).map_err(|e| format!("shard merge answered {v}: {e}"))?;
    Ok((a, budget))
}

/// The test merge Actions dispatch asks for (`design/DESIGN.md`, Shape, `demand`): `ActionsTrigger` calls it for a
/// `pull_request` event before it dispatches, with `MergeCause::Dispatch` (`crate::actions_trigger`). It reads the pull request fresh from
/// the group's leader, and if it lacks a current test merge has this site's shard of the network compute it under the
/// budget and commit it (`/demand`, as a single-PR GET does from the edge). `Ok(false)` when there was nothing to
/// compute, or the shard shed it, so `mergeable` stays null; `Ok(true)` once the merge is recorded.
pub async fn demand(
    env: &Env,
    repo: RepoId,
    n: git::issues::IssueNumber,
    cause: git::prsync::MergeCause,
) -> std::result::Result<bool, String> {
    let chain = ChainId::Repo(repo);
    let record = record_of(env, repo).await?;
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = Bindings { env, sites: &site.sites };
    let (v, _) = graftyard_lookup::call(&b, chain, &record, "/read", &json!({ "q": { "prsync": [n] } })).await?;
    let read: git::prsync::SyncRead =
        serde_json::from_value(v["value"].clone()).map_err(|e| format!("prsync read answered {v}: {e}"))?;
    let Some((base, head)) = read.views.first().and_then(git::prsync::PrView::needs_merge) else { return Ok(false) };
    let rec = js_sys::encode_uri_component(&serde_json::to_string(&record).map_err(|e| e.to_string())?);
    let url = format!(
        "https://shard/demand?repo={}&n={}&base={}&head={}&cause={}&record={rec}",
        repo.0.0,
        n.0,
        base.hex(),
        head.hex(),
        cause.as_str()
    );
    let v = shard_post(env, &site, repo, &url).await?.0;
    Ok(v["committed"].as_bool() == Some(true))
}

/// A POST to this site's shard of the network, with the budget counts it sent (`x-graft-budget`).
async fn shard_post(env: &Env, site: &SiteEnv, repo: RepoId, url: &str) -> std::result::Result<(Value, Value), String> {
    let me = site.id;
    let region = site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", me.0))
        .and_then(|ns| {
            ns.get_by_name_with_location_hint(
                &git::first_shard_name(graftyard_types::NetworkId::of_root(repo), me),
                region.hint(),
            )
        })
        .map_err(|e| e.to_string())?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let req = Request::new_with_init(url, &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("shard {}: {} {text}", url.split('?').next().unwrap_or(""), r.status_code()));
    }
    let budget = r
        .headers()
        .get("x-graft-budget")
        .ok()
        .flatten()
        .and_then(|h| serde_json::from_str(&h).ok())
        .unwrap_or(Value::Null);
    let v = serde_json::from_str(&text).map_err(|e| format!("shard answered {text}: {e}"))?;
    Ok((v, budget))
}

/// The leader's answer to the consumer's read: the cursor and each pull request's view, from the core tier, with the
/// `mergeable` GraphQL renders (`graftyard_git::prsync::mergeable`), for BF-24's driver.
pub fn read_json(fold: &git::repo::RepoFold, prs: &Value) -> Value {
    let prs: Vec<git::issues::IssueNumber> = serde_json::from_value(prs.clone()).unwrap_or_default();
    let repo = fold.repo.unwrap_or(RepoId(DbId(0)));
    json!(git::prsync::SyncRead::of(&fold.core, repo, &prs))
}

/// Test installs: opens pull request `n` from `head` to `base` by recording its `PrOpened`, since the edge serves no
/// pull request route yet (M2-WP2's handlers are not wired into `workers/edge`). The capture reads both refs from
/// the fold; nothing else checks it.
#[cfg(feature = "test-faults")]
pub async fn open_pr(h: &Rc<Host<Repo>>, body: &Value) -> Result<Value> {
    use git::issues::{HeadSpec, IssueCapture, IssueNumber, PrCapture, PrState};
    let name = |k: &str| {
        git::refs::RefName::parse(body[k].as_str().unwrap_or("").as_bytes())
            .map_err(|e| Error::RustError(e.to_string()))
    };
    let (head, base) = (name("head")?, name("base")?);
    let n = IssueNumber(body["n"].as_u64().ok_or_else(|| crate::host::err("n"))? as u32);
    let ev = {
        let core = h.core.borrow();
        let c = core.as_ref().ok_or_else(|| crate::host::err("not joined"))?;
        let fold = &c.m.group_fold().fold;
        let get = |r: &git::refs::RefName| -> Option<git::refs::ObjectId> {
            git::tiers::get_rec(&*fold.core, "ref", &[git::tiers::ref_key(r)])
        };
        let (Some(head_sha), Some(base_sha)) = (get(&head), get(&base)) else {
            return Ok(json!({ "refused": "head or base missing" }));
        };
        let repo = fold.repo.ok_or_else(|| crate::host::err("no repository"))?;
        let issue = IssueCapture {
            id: DbId((1 << 42) + i64::from(n.0)),
            author: None,
            title: format!("bf24 #{}", n.0).into(),
            body: "".into(),
            labels: Default::default(),
            assignees: Default::default(),
        };
        let capture = PrCapture {
            head_sha,
            base_sha,
            state: PrState::Open { draft: false },
            labels: Vec::new(),
            issue,
            changes_from: None,
        };
        git::repo::RepoEvent::PrOpened { n, head: HeadSpec { repo, branch: head }, base, capture }
    };
    let rid = graftyard_raft::RequestId(graftyard_types::Ulid((0x4f50u128 << 112) | u128::from(n.0)));
    let rx = h.wait_recorded(rid);
    if let Err(e) = h.input(|m, t| m.record(t, rid, git::repo::REPO_FORMAT_V1, vec![ev]))? {
        return Ok(json!({ "error": crate::host::call_error_json(&e) }));
    }
    h.served();
    match crate::host::within(std::time::Duration::from_secs(10), rx).await {
        Some(Ok(Ok(index))) => Ok(json!({ "recorded": index.0 })),
        Some(Ok(Err(nl))) => Ok(json!({ "error": crate::host::nl_json(&nl) })),
        _ => Ok(json!({ "busy": true })),
    }
}
