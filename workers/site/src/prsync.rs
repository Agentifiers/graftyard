//! The `PrSync` consumer on a site script (`design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, "Dominant access
//! patterns", Webhook: leader alarm, cursor, Queue message, consumer Worker). The pure half is
//! `graftyard_git::prsync`.
//!
//! - **Leader alarm.** A repository member that leads runs its `Dispatcher` on the alarm after it applies entries. It
//!   reads the log from the group's committed `PrSync` cursor. Each batch's first attempt runs in band: the alarm
//!   spawns the consumer's handler (`one`) in this isolate, in the background, so the alarm, the member's Raft inputs,
//!   and its replies never wait on the git cache. One first attempt runs at a time per member; batches meanwhile wait
//!   and go out as one. Work the cursor has not passed `REDELIVER_AFTER_MS` after it was handed out is sent again
//!   through the site's queue (`PRSYNC`), as is a first attempt that failed. The dispatcher lives in this isolate's
//!   memory for one leadership, so a reset member or a new leader starts again at the cursor.
//! - **Consumer.** `one` is the handler for both paths. It reads each batch's pull requests fresh from the group's
//!   leader, has the network's git cache shard on its own site compute every missing test merge (`/merge`), and
//!   commits the batch's `PrSync` commands under their deterministic request ids. The queue handler acks a message
//!   only once its last command, which moves the cursor, committed; anything else is retried by Queues. Neither path
//!   is needed for correctness: a duplicate commits nothing, and work no attempt finished is covered by the cursor and
//!   the re-send. No path calls the Cloudflare API: the first attempt uses the member's own bindings, the re-send the
//!   queue binding.

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

/// A first attempt with no answer by then counts as failed, and its batch goes to the queue at once [guess: the
/// lookup's own 20 s, plus a cold shard's catch-up].
const FIRST_FOR: std::time::Duration = std::time::Duration::from_secs(25);

/// Which leadership a dispatcher belongs to: the term, and the member's boot, so an object reset and restored in the
/// same isolate and term (a `/reset`, a failed write) starts a new one and waits on no attempt of its old self.
type Leadership = (u64, u64);

thread_local! {
    /// Per member object of this isolate: the leadership its dispatcher belongs to, and the dispatcher.
    static DISPATCHERS: RefCell<HashMap<String, (Leadership, Dispatcher)>> = RefCell::default();
    /// Test installs: first attempts to drop, per member object, as if each was lost with its isolate.
    #[cfg(feature = "test-faults")]
    static DROP_FIRST: RefCell<HashMap<String, u32>> = RefCell::default();
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
    let mut d = DISPATCHERS.with(|m| match m.borrow().get(&key) {
        Some((t, d)) if *t == term => d.clone(),
        _ => Dispatcher::default(),
    });
    if let Err(e) = scan(h, &mut d, repo, cursor, applied) {
        note(h, format!("prsync scan: {e}"));
    }
    // A scan that lost a fold read may have missed a pull request an entry touched; a batch past it would skip it.
    if h.fold_lost() {
        return;
    }
    let batch = d.batch(cursor, Timestamp(now_ms()));
    let mut first = None;
    match batch {
        Some((b, Delivery::First)) => first = Some(Message { repo: repo.0.0, batch: b, record: Some(record) }),
        Some((b, Delivery::Again)) => {
            let m = Message { repo: repo.0.0, batch: b, record: Some(record) };
            if let Err(e) = enqueue(h, &m).await {
                d.unsent();
                note(h, format!("prsync send {}..{}: {e}", m.batch.from.0, m.batch.through.0));
            }
        }
        None => {}
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
    if let Some(m) = first {
        h.stats.borrow_mut().prsync_first += 1;
        // Weak, as `Host::kick`'s loop: a member reset while its attempt runs must not stay alive for it (F47).
        let w = Rc::downgrade(h);
        let env = h.env.clone();
        spawn_local(async move {
            let t0 = now_ms();
            let r = crate::host::within(FIRST_FOR, one(&env, &m))
                .await
                .unwrap_or_else(|| Err(format!("no answer in {} s", FIRST_FOR.as_secs())));
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
            // Batches that waited for this attempt, or the re-send of a failed one.
            h.arm_in(LEADER_ALARM_MS);
        });
    }
    if waiting {
        h.arm_in(WAIT_MS);
    }
}

/// The re-send path: one message on this site's queue.
async fn enqueue(h: &Rc<Host<Repo>>, m: &Message) -> Result<()> {
    h.env.queue("PRSYNC")?.send(m.clone()).await?;
    h.stats.borrow_mut().prsync_sent += 1;
    Ok(())
}

fn scan(h: &Rc<Host<Repo>>, d: &mut Dispatcher, repo: RepoId, cursor: Index, applied: Index) -> Result<()> {
    let core = h.core.borrow();
    let c = core.as_ref().ok_or_else(|| crate::host::err("not joined"))?;
    let fold = &c.m.group_fold().fold;
    let sql = h.state.storage().sql();
    // Entries archived and truncated before this release ran its first `PrSync` are not read: they predate the
    // consumer (`tests/m2/results/m2-wp5.md`, "Design gaps"). Entries in memory are read from the member, older ones
    // from the stored log.
    let from = cursor.max(Index(fold.first_index().0.saturating_sub(1)));
    git::prsync::scan(d, &fold.core, repo, from, applied, |i| {
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
                Err(e) => {
                    console_error!("mergequeue {} {}..{}: {e}", q.repo, q.batch.from.0, q.batch.through.0);
                    m.retry();
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
        match one(&env, &body).await {
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
            Err(e) => {
                console_error!("prsync {} {}..{}: {e}", body.repo, body.batch.from.0, body.batch.through.0);
                m.retry();
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

thread_local! {
    /// Each repository's test-merge budget on this isolate, at k = 1 until `fanout` is wired to the consumer.
    static BUDGETS: RefCell<HashMap<i64, git::prsync::MergeBudget>> = RefCell::default();
}

async fn one(env: &Env, m: &Message) -> std::result::Result<Value, String> {
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
    let mut merges = Vec::new();
    let mut shed = Vec::new();
    for view in &views {
        let Some((base, head)) = view.needs_merge() else { continue };
        // TEST_MERGE_BUDGET: a head push's merge is shed only past the budget by more than one merge.
        let est = git::prsync::MERGE_EST_MS;
        let ok = BUDGETS.with(|b| {
            let mut b = b.borrow_mut();
            let b = b.entry(repo.0.0).or_insert_with(|| git::prsync::MergeBudget::new(1, now_ms()));
            b.admit(git::prsync::MergeCause::HeadPush, view.n, est, now_ms())
        });
        if !ok {
            shed.push((view.n, head));
            continue;
        }
        let answer = merge(env, &site, repo, base, head).await?;
        merges.push(answer.test_merge(view.n, base, head).ok_or("the shard named no object id")?);
    }
    t.push(now_ms());
    let cmds = git::prsync::commands(&m.batch, &views, merges, shed);
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
    Ok(json!({ "prs": views.len(), "events": events, "ms": steps }))
}

/// The test merge on this site's shard of the network. A site script binds only its own `GitShard` class
/// (`workers/site/deploy.mjs`), and the queue is the leader's site's, so the merge runs where the leader is; a shard
/// that does not hold the network yet rehydrates it from the log and the buckets, as any shard does.
async fn merge(
    env: &Env,
    site: &SiteEnv,
    repo: RepoId,
    base: git::refs::ObjectId,
    head: git::refs::ObjectId,
) -> std::result::Result<git::prsync::MergeAnswer, String> {
    let me = site.id;
    let region = site.sites.get(me).ok_or("this site is not in GRAFTYARD_SITES")?.region;
    let stub = env
        .durable_object(&format!("SHARD_{}", me.0))
        .and_then(|ns| ns.get_by_name_with_location_hint(&format!("git#{}#net:{}", me.0, repo.0.0), region.hint()))
        .map_err(|e| e.to_string())?;
    let url = format!("https://shard/merge?repo={}&base={}&head={}", repo.0.0, base.hex(), head.hex());
    let mut init = RequestInit::new();
    init.with_method(Method::Post);
    let req = Request::new_with_init(&url, &init).map_err(|e| e.to_string())?;
    let mut r = stub.fetch_with_request(req).await.map_err(|e| e.to_string())?;
    let text = r.text().await.map_err(|e| e.to_string())?;
    if r.status_code() != 200 {
        return Err(format!("shard merge: {} {text}", r.status_code()));
    }
    serde_json::from_str(&text).map_err(|e| format!("shard merge answered {text}: {e}"))
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
