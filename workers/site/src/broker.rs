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
//! - **State** is the held polls only, in the isolate's memory; a broker that restarts loses held polls, and the runners
//!   poll again under their sessions, which are entries in the group (BF-55).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use futures_channel::oneshot;
use graftyard_types::{ChainId, PlacementRecord};
use serde_json::{Value, json};
use worker::*;

/// Held long polls per broker object (DESIGN: 2,000 [guess]).
pub const BROKER_HELD_MAX: u32 = 2_000;
/// A poll is held this long [inference: under the runner's 100 s HTTP timeout, measured].
const POLL_HOLD: Duration = Duration::from_secs(50);
/// A held poll asks the leader again this often without a hint [guess: SHARED's reload interval is 30 s; a short
/// interval bounds a lost hint's delay].
const ASK_EVERY: Duration = Duration::from_secs(5);

#[durable_object]
pub struct RunnerBroker {
    env: Env,
    held: Cell<u32>,
    /// The job each runner was last handed by this object, until its cancellation is sent or it is handed another, so a
    /// runner whose job was cancelled gets `JobCancellation` once (in memory: a restarted broker sends none, and the
    /// lease alarm ends the job).
    handed: RefCell<std::collections::BTreeMap<String, Value>>,
    /// Polls waiting for a hint: woken all at once when one arrives.
    waiters: RefCell<Vec<oneshot::Sender<()>>>,
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
        let r = self.hold(b).await;
        self.held.set(self.held.get() - 1);
        Response::from_json(&r)
    }

    async fn hold(&self, b: Value) -> Value {
        let chain: Option<ChainId> = b["chain"].as_str().and_then(|c| c.parse().ok());
        let record: Option<PlacementRecord> = serde_json::from_value(b["record"].clone()).ok();
        let (Some(chain), Some(record)) = (chain, record) else { return json!({ "status": 400, "body": null }) };
        let end = crate::now_ms() + POLL_HOLD.as_millis() as u64;
        let who = b["grant"].to_string();
        let mut ask = json!({ "op": "match", "grant": b["grant"] });
        loop {
            let held = self.handed.borrow().get(&who).cloned();
            if let Some(j) = held
                && let Ok(v) =
                    self.ask(chain, &record, &json!({ "op": "cancelled", "grant": b["grant"], "job": j })).await
                && !v["body"].is_null()
            {
                self.handed.borrow_mut().remove(&who);
                return v;
            }
            match self.ask(chain, &record, &ask).await {
                Ok(v) if !v["body"].is_null() || v["status"] != 200 => {
                    if let Some(j) = v["job"].as_i64() {
                        self.handed.borrow_mut().insert(who, json!(j));
                    }
                    return v;
                }
                Ok(_) => {}
                Err(_) => ask["retry"] = json!(true),
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
}

impl DurableObject for RunnerBroker {
    fn new(_state: State, env: Env) -> Self {
        RunnerBroker {
            env,
            held: Cell::new(0),
            handed: RefCell::new(Default::default()),
            waiters: RefCell::new(Vec::new()),
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
            "/stats" => Response::from_json(&json!({ "held": self.held.get() })),
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
