//! `actions:<owner>.<shard>` members: an owner's Actions shard (`design/DESIGN.md`, Shape, `ActionsEvent`;
//! `design/SHARED.md`, "Actions runs and jobs"). The fold and commands are `graftyard_actions::shard`.
//!
//! - **Stored** as one blob, as `Authz` is: a shard's rows are small and short-lived (`tests/m4/results/
//!   m4-wp3-design.md`, The Actions chain).
//! - **The lease alarm.** After the leader applies entries, and on each of its alarms, it commits `ExpireLeases` once the
//!   fold's earliest lease end or queue deadline has passed, then arms for the next one. The request id is the chain and
//!   the deadline, so an alarm run twice commits once. This is BF-4's half on the site.
//! - **Reads** (`/read` `q`): `{"job": id}` (the row), `{"run": id}` (the run and its jobs), `{"runner": id}` (the record,
//!   with its key, for the OAuth exchange), `{"queued": true}` (the broker's summary), and otherwise the counts.

use std::rc::Rc;

use graftyard_actions::shard::{
    ActionsActor, ActionsCmd, ActionsErr, ActionsEvent, ActionsFold, ActionsOp, ActionsOut,
};
use graftyard_actions::{JobId, RunId, RunnerId, shard as act};
use graftyard_raft::{self as raft, Format, Role, codec};
use graftyard_types::{ChainId, DbId, RequestId, Timestamp, Ulid};
use serde_json::{Value, json};
use worker::SqlStorage;

use crate::family::{Family, Source, Stmt, load_blob, refusal, store_blob};
use crate::host::Host;
use crate::member::Join;

pub struct Actions;

/// How soon after applying entries the leader checks the deadlines: the earliest is at least a lease or a queue limit
/// away from any entry that set it, so a second is ample.
const CHECK_AFTER_MS: i64 = 1_000;

fn id_of(q: &Value, k: &str) -> Option<DbId> {
    q.get(k).and_then(Value::as_i64).map(DbId)
}

/// A request id the tag `0x4158` ("AX") in the top 16 bits, then 112 bits of SHA-256 of `parts`.
fn rid(parts: &str) -> RequestId {
    use sha2::Digest as _;
    let h = sha2::Sha256::new().chain_update(parts).finalize();
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x4158u128 << 112) | low))
}

impl Family for Actions {
    type Ev = ActionsEvent;
    type Fold = ActionsFold;
    type C = ActionsCmd;

    fn reads() -> Vec<Format> {
        vec![act::ACTIONS_FORMAT]
    }
    fn fold_alarm_ms(leading: bool) -> Option<i64> {
        leading.then_some(CHECK_AFTER_MS)
    }
    async fn on_alarm(host: &Rc<Host<Self>>) {
        expire(host).await;
    }
    fn new_fold(_sql: &SqlStorage, _join: &Join) -> ActionsFold {
        ActionsFold::default()
    }
    fn store_fold(fold: &ActionsFold, _: &[&raft::LogEntry<ActionsEvent>], stmts: &mut Vec<Stmt>) {
        store_blob(fold, stmts);
    }
    fn load_fold(sql: &SqlStorage, join: &Join) -> worker::Result<ActionsFold> {
        Ok(load_blob(sql)?.unwrap_or_else(|| Self::new_fold(sql, join)))
    }
    fn out_json(_fold: &ActionsFold, a: &raft::Acked<ActionsOut>) -> Value {
        serde_json::to_value(&a.out).unwrap_or(Value::Null)
    }
    fn err_json(e: &ActionsErr) -> Value {
        match e {
            ActionsErr::Forbidden => refusal(e, 403),
            ActionsErr::NotFound => refusal(e, 404),
            ActionsErr::Stale(_) | ActionsErr::Duplicate => refusal(e, 409),
            _ => refusal(e, 422),
        }
    }
    fn read_json(f: &ActionsFold, q: &Value) -> Value {
        if let Some(j) = id_of(q, "job") {
            return serde_json::to_value(f.job(JobId(j))).unwrap_or(Value::Null);
        }
        if let Some(r) = id_of(q, "run") {
            let jobs: Vec<Value> =
                f.run_jobs(RunId(r)).into_iter().map(|j| serde_json::to_value(j).unwrap_or(Value::Null)).collect();
            return json!({ "run": f.run(RunId(r)), "jobs": jobs });
        }
        if let Some(r) = id_of(q, "runner") {
            let r = RunnerId(r);
            return json!({ "runner": f.runners.get(&r), "session": f.sessions.get(&r) });
        }
        if q.get("queued").and_then(Value::as_bool) == Some(true) {
            let queued: Vec<Value> = f
                .queued()
                .into_iter()
                .map(|(job, labels, since, version)| json!({ "job": job, "labels": labels, "since": since, "version": version }))
                .collect();
            return json!({ "queued": queued, "moves": f.queue_moves });
        }
        json!({
            "runs": f.runs.len(),
            "jobs": f.jobs.len(),
            "runners": f.runners.len(),
            "sessions": f.sessions.len(),
            "moves": f.queue_moves,
            "ownerCursor": f.owner_cursor.0,
            "nextDeadline": f.next_deadline(),
        })
    }
    fn fold_bytes(f: &ActionsFold) -> Vec<u8> {
        codec::to_bytes(f)
    }
    fn dep(chain: ChainId, _bytes: &[u8]) -> Result<(), String> {
        Err(format!("an Actions shard reads no dependency ({chain})"))
    }
    fn needs_block(e: &ActionsErr) -> bool {
        matches!(e, ActionsErr::NeedIdBlock)
    }
    fn source(join: &Join) -> Source {
        match join.chain {
            ChainId::Actions(o, _) => Source::Owner(o),
            _ => Source::Given,
        }
    }
}

/// The leader's lease alarm: `ExpireLeases` once the earliest deadline has passed, then an alarm at the next one.
async fn expire(h: &Rc<Host<Actions>>) {
    let now = crate::now_ms();
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        c.m.group_fold().fold.next_deadline().map(|d| (c.join.chain, d))
    };
    let Some((chain, Timestamp(due))) = found else { return };
    if due > now {
        h.arm_in((due - now) as i64);
        return;
    }
    let cmd = ActionsCmd { actor: ActionsActor::Operator, op: ActionsOp::ExpireLeases };
    let rid = rid(&format!("{chain}/expire/{due}"));
    let admitted = match h.input(|m, t| m.commit(t, rid, cmd)) {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return worker::console_error!("{chain} ExpireLeases: {}", crate::host::call_error_json(&e)),
        Err(e) => return worker::console_error!("{chain} ExpireLeases: {e}"),
    };
    h.served();
    let w = h.wait_ticket(admitted.ticket, None);
    if !matches!(crate::host::within(std::time::Duration::from_secs(10), w).await, Some(Ok(Ok(_)))) {
        h.arm_in(5_000);
    }
}
