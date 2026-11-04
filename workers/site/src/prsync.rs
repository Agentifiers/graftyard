//! The `PrSync` consumer on a site script (`design/SHARED.md`, "Event fan-out"; `design/DESIGN.md`, "Dominant access
//! patterns", Webhook: leader alarm, cursor, Queue message, consumer Worker). The pure half is
//! `graftyard_git::prsync`.
//!
//! - **Leader alarm.** A repository member that leads runs its `Dispatcher` on the alarm after it applies entries. It
//!   reads the log from the group's committed `PrSync` cursor and sends each `SyncBatch` to its own site's queue
//!   (`PRSYNC`). The dispatcher lives in this isolate's memory for one leadership, so a reset member or a new leader
//!   starts again at the cursor.
//! - **Consumer.** The site's queue handler reads each batch's pull requests fresh from the group's leader, has the
//!   network's git cache shard on its own site compute every missing test merge (`/merge`), and commits
//!   the batch's `PrSync` commands under their deterministic request ids. A message is acked only once its last
//!   command, which moves the cursor, committed. Anything else is retried by Queues, and a batch the cursor has not
//!   passed is sent again by the leader after `REDELIVER_AFTER_MS`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_git as git;
use graftyard_git::prsync::{ALARM_MS, Dispatcher, PrView, SyncBatch, WAIT_MS};
use graftyard_lookup::Bindings;
use graftyard_raft::{Index, Role};
use graftyard_types::{ChainId, DbId, RepoId, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use worker::*;

use crate::family::Repo;
use crate::host::Host;
use crate::{SiteEnv, now_ms};

/// How soon after it applies entries a leading repository member's alarm runs the dispatcher.
pub const LEADER_ALARM_MS: i64 = ALARM_MS;

thread_local! {
    /// Per member object of this isolate: the term its dispatcher belongs to, and the dispatcher.
    static DISPATCHERS: RefCell<HashMap<String, (u64, Dispatcher)>> = RefCell::default();
}

/// One queue message: a batch of one repository chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub repo: i64,
    pub batch: SyncBatch,
}

/// The leader's half, on the member's alarm.
pub async fn on_alarm(h: &Rc<Host<Repo>>) {
    let leading = {
        let core = h.core.borrow();
        core.as_ref().filter(|c| c.m.role() == Role::Leader).and_then(|c| match c.join.chain {
            ChainId::Repo(repo) => {
                let fold = &c.m.group_fold().fold;
                let cursor = git::tiers::consumer_cursor(&*fold.core, git::issues::Consumer::PrSync);
                Some((c.join.me.do_name.to_string(), c.m.term().0, repo, cursor, c.m.state().applied))
            }
            _ => None,
        })
    };
    let Some((key, term, repo, cursor, applied)) = leading else { return };
    let mut d = DISPATCHERS.with(|m| match m.borrow().get(&key) {
        Some((t, d)) if *t == term => d.clone(),
        _ => Dispatcher::default(),
    });
    if let Err(e) = scan(h, &mut d, repo, cursor, applied) {
        note(h, format!("prsync scan: {e}"));
    }
    let batch = d.batch(cursor, Timestamp(now_ms()));
    if let Some(b) = &batch {
        let m = Message { repo: repo.0.0, batch: b.clone() };
        match h.env.queue("PRSYNC") {
            Ok(q) => match q.send(m).await {
                Ok(()) => h.stats.borrow_mut().prsync_sent += 1,
                Err(e) => {
                    d.unsent();
                    note(h, format!("prsync send {}..{}: {e}", b.from.0, b.through.0));
                }
            },
            Err(e) => {
                d.unsent();
                note(h, format!("prsync queue binding: {e}"));
            }
        }
    }
    let waiting = d.waiting(cursor);
    DISPATCHERS.with(|m| m.borrow_mut().insert(key, (term, d)));
    if waiting {
        h.arm_in(WAIT_MS);
    }
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
pub async fn consume(batch: MessageBatch<Message>, env: Env) -> Result<()> {
    for m in batch.iter() {
        let m = match m {
            Ok(m) => m,
            Err(e) => {
                console_error!("prsync: a message that does not decode: {e}");
                continue;
            }
        };
        let t0 = now_ms();
        match one(&env, m.body()).await {
            Ok(v) => {
                console_log!(
                    "prsync {} {}..{} {}ms {v}",
                    m.body().repo,
                    m.body().batch.from.0,
                    m.body().batch.through.0,
                    now_ms() - t0
                );
                m.ack();
            }
            Err(e) => {
                console_error!("prsync {} {}..{}: {e}", m.body().repo, m.body().batch.from.0, m.body().batch.through.0);
                m.retry();
            }
        }
    }
    Ok(())
}

async fn one(env: &Env, m: &Message) -> std::result::Result<Value, String> {
    let repo = RepoId(DbId(m.repo));
    let chain = ChainId::Repo(repo);
    let owner = crate::member::repo_owner(env, repo).await?;
    let record = crate::member::owner_record(env, owner).await?;
    let site = SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = Bindings { env, sites: &site.sites };
    let (v, _) =
        graftyard_lookup::call(&b, chain, &record, "/read", &json!({ "q": { "prsync": m.batch.prs } })).await?;
    let (cursor, views): (Index, Vec<PrView>) =
        serde_json::from_value(v["value"].clone()).map_err(|e| format!("prsync read answered {v}: {e}"))?;
    if cursor >= m.batch.through {
        return Ok(json!({ "skipped": cursor.0 }));
    }
    let mut merges = Vec::new();
    for view in &views {
        let Some((base, head)) = view.needs_merge() else { continue };
        let answer = merge(env, &site, repo, base, head).await?;
        merges.push(answer.test_merge(view.n, base, head).ok_or("the shard named no object id")?);
    }
    let cmds = git::prsync::commands(&m.batch, &views, merges);
    let mut events = 0;
    for c in cmds {
        let rid = c.request;
        let cmd = serde_json::to_value(git::repo::RepoCmd::Sync(c)).map_err(|e| e.to_string())?;
        let done = graftyard_lookup::commit_as(&b, chain, &record, rid, cmd).await?;
        events +=
            done.pointer("/ok/out/Sync/events").and_then(Value::as_u64).ok_or_else(|| format!("commit: {done}"))?;
    }
    Ok(json!({ "prs": views.len(), "events": events }))
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
    let (cursor, views) = git::prsync::views(&fold.core, repo, &prs);
    let mergeable: Vec<Option<bool>> = views.iter().map(PrView::mergeable).collect();
    json!({ "cursor": cursor, "views": views, "mergeable": mergeable })
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
