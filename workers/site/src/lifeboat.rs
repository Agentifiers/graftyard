//! The member side of the lifeboat (`design/DESIGN.md`, "Install operations", Lifeboat; `decisions/DECISIONS.md`,
//! "`lifeboat-learner`" and "`lifeboat-bundle`"). With `GRAFTYARD_LIFEBOAT=1` the bundle runs this script's `Member`
//! class off Cloudflare, one learner per chain in the object `graftyard_lookup::lifeboat_object` names, and the lifeboat's puller feeds each
//! one committed rows through `POST /import`. No leader ever reaches it.
//!
//! - **Join.** A chain's learner joins on its first `/import`, from the record its entry 1 rebuilds
//!   (`graftyard_raft::import::record_of`), with no registry row and no source check: off Cloudflare there is no
//!   registry to read, and the importer's chain check is what holds the copy to the install's log. A repository
//!   learner takes the owner the puller names, if any. A `Release` learner verifies every entry against the install's
//!   trust anchor (`GRAFTYARD_INSTALL`, `GRAFTYARD_CUSTOMER_KEY`), which `graftyard lifeboat init` writes into the
//!   bundle's config as a site day writes it into each site script: its genesis is its entry 2, accepted there.
//! - **Import.** `Importer::feed` runs through the host's own `step`, so its writes go through the same transaction
//!   and output gate as a `/raft` call; the member's answers to the importer are dropped there. The importer resumes
//!   from the member's last stored entry after a reset.
//! - **Halt.** A row that does not chain or that `verify` refuses halts the chain: the halt is stored, alarmed, and
//!   answered to every later `/import` until a person clears it (`/import` with `{"clear": true}` is not built), and
//!   the rows before it stay committed.

use std::rc::Rc;

use graftyard_raft::import::{Halt, IMPORTER, Importer, record_of};
use graftyard_types::{AccountId, ChainId, MemberKind};
use serde::Deserialize;
use serde_json::{Value, json};
use worker::*;

use crate::family::Family;
use crate::host::{Host, err, unhex};
use crate::member::Join;

/// A lifeboat learner's learner slot: the last, so no placement's own learners share its address.
pub const SLOT: u8 = (graftyard_types::MAX_LEARNERS - 1) as u8;

/// `POST /import`'s body: the chain, rows as hex of their stored bytes from the copy's next index, and what a first
/// join needs beyond entry 1.
#[derive(Deserialize)]
pub struct ImportReq {
    pub chain: ChainId,
    pub rows: Vec<String>,
    #[serde(default)]
    pub owner: Option<AccountId>,
}

/// `join_of` for the family `h` hosts.
pub fn join_of_host<F: Family>(_h: &Rc<Host<F>>, req: &ImportReq, row1: &[u8]) -> Option<Join> {
    join_of::<F>(req, row1)
}

/// The join a lifeboat learner of `req.chain` takes, from its entry 1: `None` when that row rebuilds no record.
pub fn join_of<F: Family>(req: &ImportReq, row1: &[u8]) -> Option<Join> {
    let record = record_of::<F::Ev>(req.chain, row1)?;
    let me = graftyard_types::Member::new(req.chain, record.home, MemberKind::Learner, SLOT, record.generation);
    Some(Join {
        chain: req.chain,
        me,
        record,
        peers: Vec::new(),
        owner: req.owner,
        genesis: None,
        #[cfg(feature = "test-faults")]
        no_claim: false,
    })
}

/// Whether this script runs as the lifeboat's (`GRAFTYARD_LIFEBOAT=1`).
pub fn mode(env: &Env) -> bool {
    env.var("GRAFTYARD_LIFEBOAT").is_ok_and(|v| v.to_string() == "1")
}

const HALT_KEY: &str = "lifeboat_halt";

/// The stored halt, if the chain halted.
fn halted<F: Family>(h: &Host<F>) -> Result<Option<Value>> {
    let rows = crate::host::rows(&h.state.storage().sql(), "SELECT v FROM me WHERE k = ?", vec![HALT_KEY.into()])?;
    Ok(match rows.first() {
        Some(r) => Some(serde_json::from_str(crate::host::text(&r[0])?)?),
        None => None,
    })
}

fn halt_json(h: &Halt) -> Value {
    match h {
        Halt::Undecodable { index, why } => json!({ "undecodable": { "index": index.map(|i| i.0), "why": why } }),
        Halt::Chain { index, why } => json!({ "chain": { "index": index.0, "why": why } }),
        Halt::Refused(r) => json!({ "refused": { "index": r.index.0, "reason": r.reason } }),
    }
}

/// Steps `rows` into the joined member through the host, and stores a halt. Answers what the copy holds.
pub async fn import<F: Family>(h: &Rc<Host<F>>, rows: Vec<Vec<u8>>) -> Result<Value> {
    if let Some(halt) = halted(h)? {
        return Ok(json!({ "halt": halt, "held": held(h) }));
    }
    let mut halt = None;
    {
        let (_, out) = h.step(|m, t| {
            let mut imp = Importer::resume(m);
            let fed = imp.feed(m, t, &rows);
            halt = fed.halt;
            fed.ready
        })?;
        // The member's only peer is the importer, which is no member: its answers go nowhere.
        let mut out = out;
        out.messages.retain(|e| e.to != IMPORTER);
        out.early.retain(|e| e.to != IMPORTER);
        let _ = h.dispatch(out, None);
    }
    h.settled().await;
    if let Some(x) = &halt {
        let v = halt_json(x);
        let chain = h.core.borrow().as_ref().map(|c| c.join.chain.to_string()).unwrap_or_default();
        console_error!("lifeboat halt {chain}: {v}");
        h.state
            .storage()
            .sql()
            .exec("INSERT OR REPLACE INTO me (k, v) VALUES (?, ?)", vec![HALT_KEY.into(), v.to_string().into()])?;
        return Ok(json!({ "halt": v, "held": held(h) }));
    }
    Ok(json!({ "halt": null, "held": held(h) }))
}

/// The last entry the copy holds.
pub fn held<F: Family>(h: &Host<F>) -> u64 {
    h.core.borrow().as_ref().map_or(0, |c| c.m.state().commit.0)
}

/// The rows of a body, decoded from hex.
pub fn rows_of(req: &ImportReq) -> Result<Vec<Vec<u8>>> {
    req.rows.iter().map(|r| unhex(r).ok_or_else(|| err("a row is not hex"))).collect()
}
