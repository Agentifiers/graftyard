//! The `Audit` consumer on a site script, for `Names` and `Tokens` shards (`design/SHARED.md`, "Multi-chain
//! operations", Finisher). The pure half is `graftyard_identity::audit`.
//!
//! - **Leader alarm.** A leading member reads its log from the shard's committed `AuditCursor`, up to the first entry
//!   younger than `FINISH_AFTER`, and names what those entries owe. The batch runs in band in this isolate, one at a
//!   time per member, as `PrSync`'s first attempt does; the alarm comes back when the next entry is due. The batch's
//!   last command moves the cursor, so a batch lost with its isolate, a reset, or a new leader is run again from the
//!   cursor by whichever member leads. There is no queue re-send: a lost batch costs one alarm, which comes back
//!   after `RETRY_MS` [inference: identity groups have no consumer queue, and a batch is a few commands].
//! - **Batch.** Each owed item is planned from folds read through `fresh` (the `dep` read), its step committed through
//!   the lookup rule under a request id derived from the step, and planned again, until it owes nothing.
//! - **Off until enabled.** The cursor commands are formats 0x10.3 and 0x11.3, which a member reads only once a site
//!   day put every member of the group on this release; until then `GRAFTYARD_AUDIT` is unset and nothing runs, as
//!   the edge sends no format before `FormatsEnable` (`design/SHARED.md`, the two-release rule). A fold whose cursor
//!   moved carries a trailing part the previous release's decoder refuses, so the edge, which decodes `Names` and
//!   `Tokens` folds, must run this release first too.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use graftyard_identity as id;
use graftyard_identity::audit::{self, AuditRead, Finisher, Want};
use graftyard_identity::finish::Step;
use graftyard_raft::{Index, Role, codec};
use graftyard_types::{ChainId, RequestId, Timestamp, Ulid};
use serde_json::{Value, json};
use worker::wasm_bindgen_futures::spawn_local;
use worker::*;

use crate::family::{Family, Names, Tokens};
use crate::host::Host;
use crate::now_ms;

/// Entries one alarm reads [guess].
const SCAN_MAX: u64 = 2_000;
/// A batch with no answer by then counts as lost [guess: a few commands, each up to the lookup's 20 s].
const BATCH_FOR: std::time::Duration = std::time::Duration::from_secs(120);
/// After a failed or lost batch, the alarm comes back this soon [guess].
const RETRY_MS: i64 = 30_000;

thread_local! {
    /// Per member object of this isolate: the leadership whose batch is running.
    static RUNNING: RefCell<HashMap<String, (u64, u64)>> = RefCell::default();
}

/// Whether this site runs the consumer: `GRAFTYARD_AUDIT` set to `1`, which a test install sets once a site day put
/// every member on a release that reads 0x10.3 and 0x11.3.
fn enabled(env: &Env) -> bool {
    env.var("GRAFTYARD_AUDIT").is_ok_and(|v| v.to_string() == "1")
}

/// A family the consumer reads: what its entries owe.
pub trait Audited: Family {
    fn owed(fold: &Self::Fold, e: &graftyard_raft::LogEntry<Self::Ev>) -> Vec<audit::Owed>;
    fn cursor(fold: &Self::Fold) -> Index;
}

impl Audited for Names {
    fn owed(_fold: &id::NamesFold, e: &graftyard_raft::LogEntry<id::NamesEvent>) -> Vec<audit::Owed> {
        audit::names_owed(e)
    }
    fn cursor(fold: &id::NamesFold) -> Index {
        fold.audit_cursor()
    }
}

impl Audited for Tokens {
    fn owed(fold: &id::TokensFold, e: &graftyard_raft::LogEntry<id::TokensEvent>) -> Vec<audit::Owed> {
        audit::tokens_owed(fold, e)
    }
    fn cursor(fold: &id::TokensFold) -> Index {
        fold.audit_cursor()
    }
}

/// The leader's half, on the member's alarm.
pub async fn on_alarm<F: Audited>(h: &Rc<Host<F>>) {
    if !enabled(&h.env) {
        return;
    }
    let now = Timestamp(now_ms());
    let found = {
        let core = h.core.borrow();
        let Some(c) = core.as_ref().filter(|c| c.m.role() == Role::Leader) else { return };
        let key = c.join.me.do_name.to_string();
        let leadership = (c.m.term().0, c.m.boot());
        if RUNNING.with(|r| r.borrow().get(&key) == Some(&leadership)) {
            return;
        }
        let fold = &c.m.group_fold().fold;
        let cursor = F::cursor(fold);
        let sql = h.state.storage().sql();
        let mut owed = Vec::new();
        let scanned = audit::scan(
            cursor,
            c.m.state().applied,
            now,
            SCAN_MAX,
            |i| c.m.entry_at(i).cloned().or_else(|| crate::host::stored_entry::<F>(&sql, i)),
            |_, e| owed.extend(F::owed(fold, e)),
        );
        (key, leadership, c.join.chain, cursor, scanned, owed)
    };
    let (key, leadership, chain, cursor, scanned, owed) = found;
    let scanned = match scanned {
        Ok(s) => s,
        Err(i) => return console_error!("audit {chain}: entry {} is in neither the member nor the stored log", i.0),
    };
    if h.fold_lost() {
        return;
    }
    if let Some(due) = scanned.next_due {
        h.arm_in((due.0.saturating_sub(now.0) as i64).max(1_000));
    }
    if !audit::needs_batch(cursor, scanned.through, &owed) {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().insert(key.clone(), leadership));
    let w = Rc::downgrade(h);
    let env = h.env.clone();
    let through = scanned.through;
    spawn_local(async move {
        let t0 = now_ms();
        let r = crate::host::within(BATCH_FOR, batch(&env, chain, cursor, through, owed))
            .await
            .unwrap_or_else(|| Err(format!("no answer in {} s", BATCH_FOR.as_secs())));
        match &r {
            Ok(v) => console_log!("audit {chain} {}..{} {}ms {v}", cursor.0, through.0, now_ms() - t0),
            Err(e) => console_error!("audit {chain} {}..{}: {e}", cursor.0, through.0),
        }
        RUNNING.with(|m| {
            let mut m = m.borrow_mut();
            if m.get(&key) == Some(&leadership) {
                m.remove(&key);
            }
        });
        let Some(h) = w.upgrade() else { return };
        h.arm_in(if r.is_ok() { 1_000 } else { RETRY_MS });
    });
}

/// One batch: plan, read, and commit until it is done.
async fn batch(
    env: &Env,
    chain: ChainId,
    from: Index,
    through: Index,
    owed: Vec<audit::Owed>,
) -> std::result::Result<Value, String> {
    let mut f = Finisher::new(chain, from, through, owed);
    for _ in 0..1_000 {
        match f.want() {
            Want::Read(chains) => {
                for c in chains {
                    let fold = read(env, c).await?;
                    f.read(c, fold);
                }
            }
            Want::Submit(step) => {
                let refused = submit(env, step).await?;
                f.submitted(refused);
            }
            Want::Done(r) => return Ok(json!(r)),
            Want::Stuck(o) => return Err(format!("stuck on {o:?}")),
        }
    }
    Err("the batch did not converge".into())
}

/// A chain's fold through its leader's `fresh` (`{"q": {"dep": true}}`), or its empty fold when no group exists.
async fn read(env: &Env, chain: ChainId) -> std::result::Result<AuditRead, String> {
    let record = crate::release::record_for(env, chain).await?;
    let site = crate::SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = graftyard_lookup::Bindings { env, sites: &site.sites };
    let q = json!({ "q": { "dep": true } }).to_string();
    let bytes = match graftyard_lookup::call_opt(&b, chain, &record, "/read", q.as_bytes()).await? {
        Some((v, _, _)) => Some(
            v.get("value")
                .and_then(Value::as_str)
                .and_then(crate::host::unhex)
                .ok_or_else(|| format!("{chain}: no fold in {v}"))?,
        ),
        None => None,
    };
    let e = |x: codec::CodecError| format!("{chain}: {x}");
    Ok(match (chain, bytes) {
        (ChainId::Names(s), None) => AuditRead::Names(id::NamesFold::new(s)),
        (ChainId::Names(_), Some(b)) => AuditRead::Names(codec::from_bytes(&b).map_err(e)?),
        (ChainId::Tokens(s), None) => AuditRead::Tokens(id::TokensFold::new(s)),
        (ChainId::Tokens(_), Some(b)) => AuditRead::Tokens(codec::from_bytes(&b).map_err(e)?),
        (ChainId::Authz(o), None) => AuditRead::Authz(id::AuthzFold::new(o)),
        (ChainId::Authz(_), Some(b)) => AuditRead::Authz(codec::from_bytes(&b).map_err(e)?),
        (ChainId::Identity(a), None) => AuditRead::Identity(id::IdentityFold::new(a)),
        (ChainId::Identity(_), Some(b)) => AuditRead::Identity(codec::from_bytes(&b).map_err(e)?),
        (other, _) => return Err(format!("audit reads no {other}")),
    })
}

/// Commits a step under a request id derived from it, so a batch run twice answers the second from the first's
/// `request` row. `Ok(true)` when the leader refused it.
async fn submit(env: &Env, step: Step) -> std::result::Result<bool, String> {
    let (chain, cmd) = match &step {
        Step::Names(c, cmd) => (*c, serde_json::to_value(cmd)),
        Step::Tokens(c, cmd) => (*c, serde_json::to_value(cmd)),
        Step::Authz(c, cmd) => (*c, serde_json::to_value(cmd)),
        Step::Identity(c, cmd) => (*c, serde_json::to_value(cmd)),
    };
    let cmd = cmd.map_err(|e| e.to_string())?;
    let record = crate::release::record_for(env, chain).await?;
    let site = crate::SiteEnv::read(env).map_err(|e| e.to_string())?;
    let b = graftyard_lookup::Bindings { env, sites: &site.sites };
    graftyard_lookup::create(&b, chain, &record, &json!({})).await?;
    let done = graftyard_lookup::commit_as(&b, chain, &record, request_id(chain, &cmd), cmd).await?;
    if done.get("ok").is_some() {
        Ok(false)
    } else if done.get("refused").is_some() {
        Ok(true)
    } else {
        Err(format!("{chain}: {done}"))
    }
}

/// A step's request id: the tag `0x4155` ("AU") in the top 16 bits, which no edge-minted id has before the year
/// 10889, then 112 bits of SHA-256 over the chain and the command.
fn request_id(chain: ChainId, cmd: &Value) -> RequestId {
    use sha2::Digest as _;
    let h = sha2::Sha256::new().chain_update(chain.to_string()).chain_update(cmd.to_string()).finalize();
    let low = u128::from_be_bytes(h[..16].try_into().expect("16 bytes")) & ((1u128 << 112) - 1);
    RequestId(Ulid((0x4155u128 << 112) | low))
}
