//! The edge-to-member boundary for the repository chain: the JSON of `POST /commit`, `/outcome`,
//! `/outcome_by_request`, and `/read` on a member object, in the shapes `workers/site` already serves for every family
//! (`workers/site/src/member.rs`, `host::outcome_json`, `host::call_error_json`):
//!
//! - commit request `{"rid": "<ulid>", "cmd": <Push>}`, where a `Push`'s serde form is the base64 of `Push::to_wire`;
//! - reply `{"done": {"ok": {"index", "out", "quorum"}}}`, `{"done": {"refused": {"err", "status", "index"}}}`,
//!   `{"done": {"notLeader": {..}}}`, `{"admitted": {"term", "boot", "seq"}}`, or `{"error": {"unsupported":
//!   {"family", "version"}}}` / `{"error": {"tooLarge": {"len", "max"}}}` / `{"error": {"notLeader": {..}}}`.
//!
//! `out` for `Push` is `PushReport`: the outcomes and the report-status bytes the member rendered from its `&Acked`
//! (`smart::report_status`), because `Acked` cannot cross a binding. The edge wraps those bytes in side-band framing
//! and adds nothing, so no edge path writes an `ok` line. The edge speaks only these types, so it calls the stand-in
//! group (`standin`) today and a site member's `Repo` slot later by changing the binding.

use graftyard_raft::admit::{ActorKey, Admit, AdmitClass};
use graftyard_raft::{Acked, CallError, CommitError, NotLeader, Ticket};
use serde::{Deserialize, Serialize};

use crate::pack::b64;
use crate::repo::{Push, PushErr, PushOut, RefOutcome};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommitReq {
    /// The request id's 26-character ULID text, minted once per client request and reused on every re-send.
    pub rid: String,
    pub cmd: Push,
}

/// What the edge sends beside a command it classed other than Human (`crate::agent::admit_class`), as `"admit"` in a
/// `/commit` body; the member then calls `Member::commit_as` with it. A command classed Human carries none and the
/// member calls `Member::commit`, so a default command's body, and its admission, are what they were before AM1-WP2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmitWire {
    pub class: ClassWire,
    /// `ActorKey`: a user's account id, or an App's id with the top bit set.
    pub actor: Option<u64>,
    pub push: bool,
    pub inline: usize,
    pub placement_ok: bool,
    /// What is left of AGENT_PUSH_BOUND at this send, in ms (`Admit::budget`): relative, so the edge's clock and the
    /// member's never meet. Absent for a command with no bound, and from an edge built before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassWire {
    Human,
    MergeQueue,
    Bot,
    Agent,
}

impl AdmitWire {
    /// The edge's admission for a command by `p` in `class`, or `None` for Human, which `Member::commit` takes.
    pub fn of(class: AdmitClass, p: &graftyard_identity::Principal, push: bool, inline: usize) -> Option<AdmitWire> {
        let class = match class {
            AdmitClass::Human => return None,
            AdmitClass::MergeQueue => ClassWire::MergeQueue,
            AdmitClass::Bot => ClassWire::Bot,
            AdmitClass::Agent => ClassWire::Agent,
        };
        let actor = match p.actor {
            graftyard_identity::Actor::User(u) => Some(u.0.0.0 as u64),
            graftyard_identity::Actor::App { app, .. } => Some(1 << 63 | app.0.0 as u64),
            graftyard_identity::Actor::Anonymous => None,
        };
        Some(AdmitWire { class, actor, push, inline, placement_ok: true, budget_ms: None })
    }

    pub fn admit(self) -> Admit {
        let class = match self.class {
            ClassWire::Human => AdmitClass::Human,
            ClassWire::MergeQueue => AdmitClass::MergeQueue,
            ClassWire::Bot => AdmitClass::Bot,
            ClassWire::Agent => AdmitClass::Agent,
        };
        Admit {
            class,
            actor: self.actor.map(ActorKey),
            push: self.push,
            inline: self.inline,
            placement_ok: self.placement_ok,
            budget: self.budget_ms.map(std::time::Duration::from_millis),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketWire {
    pub term: u64,
    /// A `u64` drawn at random, as text so JSON keeps every bit.
    pub boot: String,
    pub seq: u64,
}

impl From<Ticket> for TicketWire {
    fn from(t: Ticket) -> TicketWire {
        TicketWire { term: t.term.0, boot: t.boot.to_string(), seq: t.seq }
    }
}

impl TicketWire {
    pub fn ticket(&self) -> Option<Ticket> {
        Some(Ticket { term: graftyard_raft::Term(self.term), boot: self.boot.parse().ok()?, seq: self.seq })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotLeaderWire {
    pub term: u64,
    pub leader: Option<u8>,
}

impl From<&NotLeader> for NotLeaderWire {
    fn from(n: &NotLeader) -> NotLeaderWire {
        NotLeaderWire { term: n.term.0, leader: n.leader.map(|s| s.0) }
    }
}

/// `Push`'s `out` on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushReport {
    pub refs: Vec<RefOutcome>,
    /// Base64 of `smart::report_status(&acked)`.
    pub status: String,
}

impl PushReport {
    /// `None` for an output that is not a push's.
    pub fn of<T: crate::smart::PushOutOf>(acked: &Acked<T>) -> Option<PushReport> {
        let refs = acked.out.push_out()?.outcomes();
        Some(PushReport { refs, status: b64::encode(&crate::smart::report_status(acked)?) })
    }

    pub fn status_bytes(&self) -> Option<Vec<u8>> {
        b64::decode(&self.status)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkWire {
    pub index: u64,
    pub out: PushReport,
    /// The sites whose acks made the quorum.
    pub quorum: Vec<u8>,
}

/// `{"err", "status"}` as `Family::err_json` renders a refusal, plus its `index`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedWire {
    pub err: String,
    pub status: u16,
    pub index: u64,
}

impl RefusedWire {
    pub fn of(e: &PushErr, index: u64) -> RefusedWire {
        let (err, status) = match e {
            PushErr::Unauthorized(r) => (format!("unauthorized: {r:?}"), 401),
            PushErr::Forbidden => ("forbidden".into(), 403),
            PushErr::TooManyRefs { got } => (format!("too many refs: {got}"), 422),
            PushErr::Duplicate(n) => (format!("duplicate ref {}", n.display()), 422),
            PushErr::Agent(a) => (a.text.to_string(), 422),
        };
        RefusedWire { err, status, index }
    }
}

/// Agent admission refused the command: the edge renders `smart::Refusal::agent_full` for git and
/// `Reply::secondary_rate_limit` for REST. `{"full": {"retry_after_s": n}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FullWire {
    pub retry_after_s: u32,
}

/// A push's outcome, once its entry committed or its leadership ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushOutcome {
    Ok(OkWire),
    Refused(RefusedWire),
    NotLeader(NotLeaderWire),
    /// It waited AGENT_WAIT in the agent lane.
    Full(FullWire),
}

pub fn outcome_wire(o: &Result<Acked<PushOut>, CommitError<PushErr>>) -> PushOutcome {
    match o {
        Ok(a) => PushOutcome::Ok(OkWire {
            index: a.index.0,
            out: PushReport::of(a).expect("a push's output"),
            quorum: a.quorum().sites().iter().map(|s| s.0).collect(),
        }),
        Err(CommitError::Refused { index, err }) => PushOutcome::Refused(RefusedWire::of(err, index.0)),
        Err(CommitError::NotLeader(n)) => PushOutcome::NotLeader(n.into()),
        Err(CommitError::Full { retry_after_s }) => PushOutcome::Full(FullWire { retry_after_s: *retry_after_s }),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedWire {
    pub family: u16,
    pub version: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TooLargeWire {
    pub len: usize,
    pub max: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallErrorWire {
    NotLeader(NotLeaderWire),
    /// The member's release does not read `family.version`. The edge answers 503 with `Retry-After`.
    Unsupported(UnsupportedWire),
    TooLarge(TooLargeWire),
    /// `AdmitError::Stage` (`Member::commit_as`): store the pack in the buckets and send again under the same request
    /// id. Only a command sent with `admit` gets it. `{"stage": true}`.
    Stage(bool),
}

impl From<&CallError> for CallErrorWire {
    fn from(e: &CallError) -> CallErrorWire {
        match e {
            CallError::NotLeader(n) => CallErrorWire::NotLeader(n.into()),
            CallError::Unsupported(u) => {
                CallErrorWire::Unsupported(UnsupportedWire { family: u.format.family, version: u.format.version })
            }
            CallError::TooLarge { len, max } => CallErrorWire::TooLarge(TooLargeWire { len: *len, max: *max }),
        }
    }
}

/// What `POST /commit` answers: the outcome when the ticket resolved within COMMIT_HOLD, else the ticket, or the
/// member's refusal to take the command at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommitReply {
    Done(PushOutcome),
    Admitted(TicketWire),
    Error(CallErrorWire),
}

/// `POST /outcome` with `{ticket, wait_ms}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeReq {
    pub ticket: TicketWire,
    pub wait_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutcomeReply {
    Done(PushOutcome),
    Pending(TicketWire),
    /// `outcome_by_request` only: no row and nothing queued, so the edge re-sends. `{"unknown": true}`.
    Unknown(bool),
    Error(CallErrorWire),
}

/// `POST /read` answers `{"value": .., "index": n}` as the member's `fresh` read does; for the repository fold the
/// value is the refs as `[name, oid]` pairs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadValue {
    pub value: Vec<(String, String)>,
    pub index: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReadReply {
    Refs(ReadValue),
    Error { error: CallErrorWire },
}

/// One page of a repository's refs (`refs_read`): the refs as `[name, oid]` pairs in name order, and the name to read
/// the next page after, as base64 of its bytes, when more follow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefsPage {
    pub refs: Vec<(String, String)>,
    pub next: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RefsReply {
    Page { value: RefsPage, index: u64 },
    Error { error: CallErrorWire },
}

/// A paged ref read's query, `{"q": {"refs": {"after": b64, "prefix": b64, "limit": n}}}`: names start with `prefix`
/// and sort after `after`, each the base64 of a name's bytes, and `limit` is capped at `REFS_PAGE_MAX`.
pub fn refs_query(prefix: &[u8], after: Option<&str>, limit: usize) -> serde_json::Value {
    serde_json::json!({ "q": { "refs": { "prefix": b64::encode(prefix), "after": after, "limit": limit } } })
}

/// The member's answer to `refs_query` (`RefsPage`), and to `{"q": {}}`, which asks for every ref at once: the refs
/// when there are at most `REFS_PAGE_MAX`, else `{"paged": REFS_PAGE_MAX}`, so no reply grows with the ref count
/// (`repo:65536`, `tests/m3/results/wire-fix.md`).
pub fn refs_read(fold: &crate::repo::RepoFold, q: &serde_json::Value) -> serde_json::Value {
    use crate::repo::REFS_PAGE_MAX;
    let Some(r) = q.get("refs") else {
        if fold.refs_over(REFS_PAGE_MAX) {
            return serde_json::json!({ "paged": REFS_PAGE_MAX });
        }
        return serde_json::json!(fold.refs().iter().map(|(n, o)| (n.display(), o.hex())).collect::<Vec<_>>());
    };
    let bytes = |k: &str| r.get(k).and_then(serde_json::Value::as_str).and_then(b64::decode);
    let prefix = bytes("prefix").unwrap_or_default();
    let after = bytes("after");
    let limit = r
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .map_or(REFS_PAGE_MAX, |l| (l as usize).clamp(1, REFS_PAGE_MAX));
    let (page, more) = fold.refs_page(&prefix, after.as_deref(), limit);
    let next = more.then(|| page.last().map(|(n, _)| b64::encode(n.as_bytes()))).flatten();
    let refs = page.iter().map(|(n, o)| (n.display(), o.hex())).collect();
    serde_json::to_value(RefsPage { refs, next }).unwrap_or(serde_json::Value::Null)
}

/// The member host's half of `POST /commit`: the bytes that do not decode under a format this release reads answer
/// `Unsupported`, and nothing changes. The member's own JSON decode of `cmd` fails the same way; a host that catches
/// that failure answers with this.
pub fn decode_commit(body: &[u8], release: graftyard_types::ReleaseId) -> Result<CommitReq, CallErrorWire> {
    #[derive(Deserialize)]
    struct Raw {
        rid: String,
        cmd: String,
    }
    let raw: Raw = serde_json::from_slice(body).map_err(|_| unsupported_unknown())?;
    let bytes = b64::decode(&raw.cmd).ok_or_else(unsupported_unknown)?;
    match Push::from_wire(&bytes) {
        Ok(cmd) => Ok(CommitReq { rid: raw.rid, cmd }),
        Err(crate::repo::WireError::Format(format)) => {
            Err((&CallError::Unsupported(graftyard_raft::Unsupported { release, format })).into())
        }
        Err(_) => Err(unsupported_unknown()),
    }
}

/// A record whose format cannot even be read: answered as `Unsupported` of format 0.0, which no release reads.
fn unsupported_unknown() -> CallErrorWire {
    CallErrorWire::Unsupported(UnsupportedWire { family: 0, version: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An agent command that waited AGENT_WAIT crosses the member boundary as `{"full": {"retry_after_s": n}}`, which
    /// the edge renders in the `ng` or 429 shape and never as another refusal.
    #[test]
    fn an_admission_refusal_crosses_the_wire_with_its_retry_hint() {
        let o = outcome_wire(&Err(CommitError::Full { retry_after_s: 9 }));
        assert_eq!(o, PushOutcome::Full(FullWire { retry_after_s: 9 }));
        let j = serde_json::to_value(CommitReply::Done(o.clone())).unwrap();
        assert_eq!(j, serde_json::json!({ "done": { "full": { "retry_after_s": 9 } } }));
        assert_eq!(serde_json::from_value::<CommitReply>(j).unwrap(), CommitReply::Done(o));
    }
}
