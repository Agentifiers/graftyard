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
//!   (`{"summary": true}`, kept `SUMMARY_FRESH`; a shard with no group is asked again after `NO_GROUP_FRESH`; every
//!   read is cut at `SUMMARY_WAIT`), picks with `graftyard_actions::broker::Taken`, and asks the chosen shard's leader
//!   to commit `MatchOwner`. A job matched but sitting `Assigned` past `RESEND_AFTER`, its answer lost with a broker or
//!   a site, is handed to its runner again. Each ask reads the registration shard first, so a runner deleted while its
//!   poll was held is not matched, and an ephemeral runner this object saw holding a job is not matched again.
//! - **A leader move.** After each ask, a broker whose site no longer leads its shard (the lookup learned another
//!   leader) answers the poll empty with `moved`, so the edge looks the leader up again and the runner's next poll
//!   reaches the broker on the new leader's site (BF-55).
//! - **State** is the held polls and the summaries only, in the isolate's memory; a broker that restarts loses held
//!   polls, and the runners poll again under their sessions, which are entries in the group (BF-55).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use futures_channel::oneshot;
use graftyard_actions::broker::{Pick, Summary, Taken};
use graftyard_actions::runsvc::{self, Grant};
use graftyard_actions::{ACTIONS_SHARDS, LabelSet};
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
/// How long a poll waits for the shards' summary reads [guess]. A read still running then counts as failed, so a lost
/// site's members cannot hold a match: a lookup of a group with no members on the other sites answers `None` only when
/// every member says "not joined", and a lost site's members answer 503, so it ran its whole 20 s (BF-55 run 1).
const SUMMARY_WAIT: Duration = Duration::from_secs(3);
/// A shard whose read failed or timed out is asked again after this, keeping its last summary [guess].
const FAILED_FRESH_MS: u64 = 5_000;

/// What a broker knows of one shard.
enum Known {
    /// Its summary, read at the time given.
    Read(Summary),
    /// No group: the lookup found none, or a majority of its members answered "not joined".
    NoGroup,
    /// Its last read failed: the summary before it, if any.
    Failed(Option<Summary>),
}

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
    /// `Owner` scope: what this object knows of each shard, and when it learned it.
    summaries: RefCell<std::collections::BTreeMap<u8, (u64, Known)>>,
    /// `Owner` scope: the ephemeral runners this object matched or saw holding a job, which take one job each.
    taken: RefCell<Taken>,
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
            // Every ask reads the registration shard first, so a runner whose record went (its `DELETE session`, after
            // its one job) while this poll was held is not matched: the runner stopped listening, and the broker keeps
            // a poll the runner cancelled (BF-55 run 1).
            let answer = match self.ask(chain, &record, &ask).await {
                Ok(v) if v.get("owner").is_some() => self.owner_match(owner, &record, &b["grant"], &v["owner"]).await,
                a => a,
            };
            match answer {
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
        let (sums, complete) = self.summaries(owner, record).await;
        let refs: Vec<(u8, &Summary)> = sums.iter().map(|(s, x)| (*s, x)).collect();
        let urls = runsvc::Urls::new(&format!("https://{}", crate::hookseq::api_host(&self.env)));
        let picked = self.taken.borrow_mut().pick(&refs, complete, runner, &labels, ephemeral, Timestamp(now));
        match picked {
            Pick::Busy | Pick::Idle => Ok(json!({ "status": 200, "body": null })),
            Pick::Resend { shard, job } => Ok(
                json!({ "status": 200, "body": runsvc::owner_job_request(&urls, now, shard, job), "job": job, "shard": shard }),
            ),
            Pick::Match { shard, job, version } => {
                let body =
                    json!({ "op": "match_owner", "grant": grant, "job": job, "version": version, "labels": labels });
                let mut v = self.ask(ChainId::Actions(owner, shard), record, &body).await?;
                self.summaries.borrow_mut().remove(&shard);
                if !v["body"].is_null() {
                    if ephemeral {
                        self.taken.borrow_mut().matched(runner);
                    }
                    v["shard"] = json!(shard);
                }
                Ok(v)
            }
        }
    }

    /// Every shard's summary, and whether every shard with a group was read: each is read again once older than
    /// `SUMMARY_FRESH_MS` (`NO_GROUP_FRESH_MS` for a shard with no group, `FAILED_FRESH_MS` after a failed read), all
    /// at once and for at most `SUMMARY_WAIT`. A failed shard keeps its last summary but makes the set incomplete,
    /// unless a majority of its members answer "not joined", which a group that holds jobs cannot have: its entries
    /// committed on a majority of joined members [inference]. That check is what tells a lost site's 503s from a
    /// shard with no group, which the lookup cannot while it waits for the lost members (BF-55 run 1).
    async fn summaries(&self, owner: AccountId, record: &PlacementRecord) -> (Vec<(u8, Summary)>, bool) {
        let now = crate::now_ms();
        let due = |k: &Known, at: u64| {
            let fresh = match k {
                Known::Read(_) => SUMMARY_FRESH_MS,
                Known::NoGroup => NO_GROUP_FRESH_MS,
                Known::Failed(_) => FAILED_FRESH_MS,
            };
            now.saturating_sub(at) > fresh
        };
        let stale: Vec<u8> =
            (0..ACTIONS_SHARDS).filter(|s| self.summaries.borrow().get(s).is_none_or(|(at, k)| due(k, *at))).collect();
        if !stale.is_empty()
            && let Ok(site) = crate::SiteEnv::read(&self.env)
        {
            let b = graftyard_lookup::Bindings { env: &self.env, sites: &site.sites };
            let q = json!({ "q": { "summary": true } }).to_string();
            let reads = stale.iter().map(|s| {
                let (b, q) = (&b, &q);
                async move {
                    let chain = ChainId::Actions(owner, *s);
                    let call = graftyard_lookup::call_opt(b, chain, record, "/read", q.as_bytes());
                    let known = match crate::host::within(SUMMARY_WAIT, call).await {
                        Some(Ok(Some((v, _, _)))) => {
                            serde_json::from_value::<Summary>(v["value"].clone()).ok().map(Known::Read)
                        }
                        Some(Ok(None)) => Some(Known::NoGroup),
                        r => {
                            if let Some(Err(e)) = r {
                                console_error!("broker summary {chain}: {e}");
                            }
                            None
                        }
                    };
                    let known = match known {
                        Some(k) => k,
                        None => {
                            let ms = record.bootstrap_members(chain);
                            let posts =
                                ms.iter().map(|m| graftyard_lookup::post(b, m.site, &m.do_name, "/read", q.as_bytes()));
                            let unjoined = futures_util::future::join_all(posts)
                                .await
                                .iter()
                                .filter(|a| matches!(a, Some((409, _))))
                                .count();
                            if unjoined * 2 > ms.len() { Known::NoGroup } else { Known::Failed(None) }
                        }
                    };
                    (*s, known)
                }
            });
            for (s, known) in futures_util::future::join_all(reads).await {
                let mut m = self.summaries.borrow_mut();
                let known = match known {
                    Known::Failed(_) => Known::Failed(match m.remove(&s) {
                        Some((_, Known::Read(x) | Known::Failed(Some(x)))) => Some(x),
                        _ => None,
                    }),
                    k => k,
                };
                m.insert(s, (now, known));
            }
        }
        let m = self.summaries.borrow();
        let complete = (0..ACTIONS_SHARDS).all(|s| matches!(m.get(&s), Some((_, Known::Read(_) | Known::NoGroup))));
        let sums = m
            .iter()
            .filter_map(|(s, (_, k))| match k {
                Known::Read(x) | Known::Failed(Some(x)) => Some((*s, x.clone())),
                _ => None,
            })
            .collect();
        (sums, complete)
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
