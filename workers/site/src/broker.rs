//! `RunnerBroker` hosted (`design/DESIGN.md`, Shape, "Identity"; `design/SHARED.md`, "Actions runs and jobs", Label
//! matching; `decisions/DECISIONS.md`, "M4-WP3: the run service's signing key, the live-log viewer path, and how the
//! edge finds `RunnerBroker` and `LogRelay`").
//!
//! - **Where.** One object per registration shard, `broker:actions:<owner>.<shard>`, on the site the edge found the
//!   shard's leader on. A leader move makes the edge use the new site's object; the old one only drains.
//! - **A poll** (`/poll` `{chain, record, grant}`, the runner's `GET message`) is held up to `POLL_HOLD`. It first asks
//!   the leader whether the job this object last handed the runner was cancelled (`cancelled`), then for a match (`match`), which the leader commits
//!   against the job's version, so only a committed `Matched` becomes the runner's message. Between asks it waits
//!   `ASK_EVERY` or until the leader's `/hint` after a queue move wakes it. Past `BROKER_HELD_MAX` held polls it answers
//!   503 with `Retry-After: 15`.
//! - **`Owner` scope** (BF-55). The registration shard's `match` answers `owner` with the runner's labels instead of a
//!   match, since the runner takes jobs from all 16 of the owner's shards. The broker then reads each shard's summary
//!   (`{"summary": true}`, kept `SUMMARY_FRESH`; a shard with no group is asked again after `NO_GROUP_FRESH`), picks
//!   with `graftyard_actions::broker::pick`, and asks the chosen shard's leader to commit `MatchOwner`. A job matched
//!   but sitting `Assigned` past `RESEND_AFTER`, its answer lost with a broker or a site, is handed to its runner again.
//! - **A leader move.** After each ask, a broker whose site no longer leads its shard (the lookup learned another
//!   leader) answers the poll empty with `moved`, so the edge looks the leader up again and the runner's next poll
//!   reaches the broker on the new leader's site (BF-55).
//! - **State** is the held polls and the summaries only, in the isolate's memory; a broker that restarts loses held
//!   polls, and the runners poll again under their sessions, which are entries in the group (BF-55).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use futures_channel::oneshot;
use graftyard_actions::broker::{Pick, Summary, pick};
use graftyard_actions::runsvc::{self, Grant};
use graftyard_actions::{ACTIONS_SHARDS, LabelSet, RunnerId};
use graftyard_types::{AccountId, ChainId, PlacementRecord, Timestamp};
use serde_json::{Value, json};
use worker::*;

/// Held long polls per broker object (DESIGN: 2,000 [guess]).
pub const BROKER_HELD_MAX: u32 = 2_000;
/// A poll is held this long [inference: under the runner's 100 s HTTP timeout, measured].
const POLL_HOLD: Duration = Duration::from_secs(50);
/// A held poll asks the leader again this often without a hint [guess: SHARED's reload interval is 30 s; a short
/// interval bounds a lost hint's delay].
const ASK_EVERY: Duration = Duration::from_secs(5);
/// An `Owner`-scope broker reads a shard's summary again after this [guess: one read per shard per second bounds a
/// broker's reads at 16/s however many polls it holds].
const SUMMARY_FRESH_MS: u64 = 1_000;
/// A shard with no group is asked again after this [guess: SHARED's 30 s reload].
const NO_GROUP_FRESH_MS: u64 = 30_000;

#[durable_object]
pub struct RunnerBroker {
    env: Env,
    held: Cell<u32>,
    /// Polls this object took and when it took the last one, for `/stats` (BF-55 times the first poll on a new site).
    polls: Cell<u64>,
    last_poll: Cell<u64>,
    /// The job each runner was last handed by this object, until its cancellation is sent or it is handed another, so a
    /// runner whose job was cancelled gets `JobCancellation` once (in memory: a restarted broker sends none, and the
    /// lease alarm ends the job).
    /// Keyed by the grant, the job's shard and id.
    handed: RefCell<std::collections::BTreeMap<String, (u8, i64)>>,
    /// Polls waiting for a hint: woken all at once when one arrives.
    waiters: RefCell<Vec<oneshot::Sender<()>>>,
    /// `Owner` scope: each shard's summary and when it was read (`None`: no group).
    summaries: RefCell<std::collections::BTreeMap<u8, (u64, Option<Summary>)>>,
    /// `Owner` scope: the ephemeral runners this object matched, which take one job each (as the leader's `MATCHED`).
    taken: RefCell<std::collections::BTreeSet<RunnerId>>,
}

impl RunnerBroker {
    async fn ask(&self, chain: ChainId, record: &PlacementRecord, body: &Value) -> std::result::Result<Value, String> {
        // The lookup follows a member's `notLeader` to the leader.
        crate::lookup::call(&self.env, chain, record, "/runsvc", body).await.map(|(v, _)| v)
    }

    async fn poll(&self, b: Value) -> Result<Response> {
        if self.held.get() >= BROKER_HELD_MAX {
            return Response::from_json(&json!({ "status": 503, "retryAfter": 15 }));
        }
        self.held.set(self.held.get() + 1);
        self.polls.set(self.polls.get() + 1);
        self.last_poll.set(crate::now_ms());
        let r = self.hold(b).await;
        self.held.set(self.held.get() - 1);
        Response::from_json(&r)
    }

    async fn hold(&self, b: Value) -> Value {
        let chain: Option<ChainId> = b["chain"].as_str().and_then(|c| c.parse().ok());
        let record: Option<PlacementRecord> = serde_json::from_value(b["record"].clone()).ok();
        let (Some(chain @ ChainId::Actions(owner, own_shard)), Some(record)) = (chain, record) else {
            return json!({ "status": 400, "body": null });
        };
        let me = crate::SiteEnv::read(&self.env).map(|s| s.id).ok();
        let moved = || me.is_some_and(|m| graftyard_lookup::cached_leader(chain).is_some_and(|t| t.site != m));
        let end = crate::now_ms() + POLL_HOLD.as_millis() as u64;
        let who = b["grant"].to_string();
        let mut ask = json!({ "op": "match", "grant": b["grant"] });
        let mut owner_scope: Option<Value> = None;
        let mut quick = 0;
        loop {
            let held = self.handed.borrow().get(&who).copied();
            if let Some((s, j)) = held
                && let Ok(v) = self
                    .ask(
                        ChainId::Actions(owner, s),
                        &record,
                        &json!({ "op": "cancelled", "grant": b["grant"], "job": j }),
                    )
                    .await
                && !v["body"].is_null()
            {
                self.handed.borrow_mut().remove(&who);
                return v;
            }
            let answer = match &owner_scope {
                None => self.ask(chain, &record, &ask).await,
                Some(o) => self.owner_match(owner, &record, &b["grant"], o).await,
            };
            match answer {
                Ok(v) if owner_scope.is_none() && v.get("owner").is_some() => {
                    owner_scope = Some(v["owner"].clone());
                    continue;
                }
                Ok(v) if !v["body"].is_null() || v["status"] != 200 => {
                    if let Some(j) = v["job"].as_i64() {
                        let s = v["shard"].as_u64().map_or(own_shard, |s| s as u8);
                        self.handed.borrow_mut().insert(who, (s, j));
                    }
                    return v;
                }
                // A match refused for a version another poll moved: the summary was dropped, so ask again now.
                Ok(v) if v.get("refused").is_some() && quick < 4 => {
                    quick += 1;
                    continue;
                }
                Ok(_) => {}
                Err(_) => ask["retry"] = json!(true),
            }
            if moved() {
                return json!({ "status": 200, "body": null, "moved": true });
            }
            let left = end.saturating_sub(crate::now_ms());
            if left == 0 {
                return json!({ "status": 200, "body": null });
            }
            let (tx, rx) = oneshot::channel();
            self.waiters.borrow_mut().push(tx);
            let _ = crate::host::within(Duration::from_millis(left).min(ASK_EVERY), rx).await;
        }
    }

    /// One `Owner`-scope match over the owner's shards (module docs). `o` is the registration shard's `owner` answer.
    async fn owner_match(
        &self,
        owner: AccountId,
        record: &PlacementRecord,
        grant: &Value,
        o: &Value,
    ) -> std::result::Result<Value, String> {
        let (Ok(Grant::Runner { runner }), Ok(labels)) =
            (serde_json::from_value::<Grant>(grant.clone()), serde_json::from_value::<LabelSet>(o["labels"].clone()))
        else {
            return Err("an Owner-scope poll without a runner grant or labels".into());
        };
        let ephemeral = o["ephemeral"].as_bool().unwrap_or(true);
        let now = crate::now_ms();
        let sums = self.summaries(owner, record).await;
        let refs: Vec<(u8, &Summary)> = sums.iter().map(|(s, x)| (*s, x)).collect();
        let urls = runsvc::Urls::new(&format!("https://{}", crate::hookseq::api_host(&self.env)));
        match pick(&refs, runner, &labels, Timestamp(now)) {
            Pick::Busy | Pick::Idle => Ok(json!({ "status": 200, "body": null })),
            Pick::Resend { shard, job } => Ok(
                json!({ "status": 200, "body": runsvc::owner_job_request(&urls, now, shard, job), "job": job, "shard": shard }),
            ),
            Pick::Match { .. } if ephemeral && self.taken.borrow().contains(&runner) => {
                Ok(json!({ "status": 200, "body": null }))
            }
            Pick::Match { shard, job, version } => {
                let body =
                    json!({ "op": "match_owner", "grant": grant, "job": job, "version": version, "labels": labels });
                let mut v = self.ask(ChainId::Actions(owner, shard), record, &body).await?;
                self.summaries.borrow_mut().remove(&shard);
                if !v["body"].is_null() {
                    if ephemeral {
                        self.taken.borrow_mut().insert(runner);
                    }
                    v["shard"] = json!(shard);
                }
                Ok(v)
            }
        }
    }

    /// Every shard's summary, each read again once older than `SUMMARY_FRESH_MS` (`NO_GROUP_FRESH_MS` for a shard
    /// with no group), all at once. A shard that did not answer keeps its last summary.
    async fn summaries(&self, owner: AccountId, record: &PlacementRecord) -> Vec<(u8, Summary)> {
        let now = crate::now_ms();
        let stale: Vec<u8> = (0..ACTIONS_SHARDS)
            .filter(|s| match self.summaries.borrow().get(s) {
                Some((at, Some(_))) => now.saturating_sub(*at) > SUMMARY_FRESH_MS,
                Some((at, None)) => now.saturating_sub(*at) > NO_GROUP_FRESH_MS,
                None => true,
            })
            .collect();
        if !stale.is_empty()
            && let Ok(site) = crate::SiteEnv::read(&self.env)
        {
            let b = graftyard_lookup::Bindings { env: &self.env, sites: &site.sites };
            let q = json!({ "q": { "summary": true } }).to_string();
            let reads = stale.iter().map(|s| {
                let (b, q) = (&b, &q);
                async move {
                    (
                        *s,
                        graftyard_lookup::call_opt(b, ChainId::Actions(owner, *s), record, "/read", q.as_bytes()).await,
                    )
                }
            });
            for (s, r) in futures_util::future::join_all(reads).await {
                match r {
                    Ok(Some((v, _, _))) => {
                        let sum = serde_json::from_value::<Summary>(v["value"].clone()).ok();
                        self.summaries.borrow_mut().insert(s, (now, sum));
                    }
                    Ok(None) => {
                        self.summaries.borrow_mut().insert(s, (now, None));
                    }
                    Err(e) => console_error!("broker summary actions:{}.{s}: {e}", owner.0.0),
                }
            }
        }
        self.summaries.borrow().iter().filter_map(|(s, (_, x))| x.clone().map(|x| (*s, x))).collect()
    }
}

impl DurableObject for RunnerBroker {
    fn new(_state: State, env: Env) -> Self {
        RunnerBroker {
            env,
            held: Cell::new(0),
            polls: Cell::new(0),
            last_poll: Cell::new(0),
            handed: RefCell::new(Default::default()),
            waiters: RefCell::new(Vec::new()),
            summaries: RefCell::new(Default::default()),
            taken: RefCell::new(Default::default()),
        }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        if crate::site_down(&self.env) {
            return Response::error("site down", 503);
        }
        match req.path().as_str() {
            "/poll" => {
                let b: Value = req.json().await?;
                self.poll(b).await
            }
            // A queue move on the shard: every held poll asks again now.
            "/hint" => {
                for w in self.waiters.borrow_mut().drain(..) {
                    let _ = w.send(());
                }
                Response::from_json(&json!({ "held": self.held.get() }))
            }
            "/stats" => Response::from_json(
                &json!({ "held": self.held.get(), "polls": self.polls.get(), "lastPoll": self.last_poll.get() }),
            ),
            _ => Response::error("not found", 404),
        }
    }
}

/// The leader's hint after an entry that moved a job into or out of `Queued`: the broker object on this site wakes its
/// held polls. Best effort; a lost hint delays a match by at most `ASK_EVERY`.
pub async fn hint(env: &Env, site: u8, chain: ChainId) {
    let r = async {
        let stub = env.durable_object(&format!("BROKER_{site}"))?.get_by_name(&format!("broker:{chain}"))?;
        stub.fetch_with_str("https://broker/hint").await
    }
    .await;
    if let Err(e) = r {
        console_error!("broker hint {chain}: {e}");
    }
}
