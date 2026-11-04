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
        };
        RefusedWire { err, status, index }
    }
}

/// A push's outcome, once its entry committed or its leadership ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PushOutcome {
    Ok(OkWire),
    Refused(RefusedWire),
    NotLeader(NotLeaderWire),
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
