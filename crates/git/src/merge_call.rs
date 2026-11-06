//! A merge or merge-queue command as a repository member commits it, inside `crate::repo::RepoCmd::Merge`: the
//! `MergeCmd` of `crate::merge`, with the reply its leader renders from the `&Acked` (`render`, opaque here, as
//! `crate::issue_ops::call::IssueCall` carries one).
//!
//! Its JSON names format 32.4 first, so a member whose release does not read it refuses the command without reading
//! the rest. The edge sends one only once `FormatsEnable` has enabled 32.4. A pack crosses as its `PackRef`, and the
//! member trusts it as it trusts a `Push`'s (`DurablePack::trusted`): the edge or the consumer placed it.

use graftyard_identity::{Actor, Credential, OrgId, Principal, ScopeSet, UserId};
use graftyard_types::{AccountId, DbId, Digest, Index};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use smallvec::SmallVec;

use crate::issues::{IssueNumber, MergeMethod};
use crate::merge::{
    EngineVersion, Merge, MergeCmd, MergeCmdOut, MergeError, MergePlan, REPO_FORMAT_V4, TreeDiffDigest,
};
use crate::pack::{DurablePack, PackRef};
use crate::queue::{EjectReason, MergeQueueEntry, QueueAction, QueueMerge, QueueOp};
use crate::refs::{ObjectId, RefName};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCall {
    pub cmd: MergeCmd,
    /// What the leader renders from the `&Acked` (`graftyard_api::hosted::Render`); `Null` for a consumer's command.
    pub render: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeCallOut {
    pub out: MergeCmdOut,
    pub render: Value,
}

#[derive(Serialize, Deserialize)]
struct PrincipalWire {
    account: Option<i64>,
    scopes: u64,
    token: Digest,
    sso: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<graftyard_types::Ulid>,
}

impl PrincipalWire {
    fn of(p: &Principal) -> PrincipalWire {
        let (token, session) = p.credential.wire();
        PrincipalWire {
            session,
            account: match p.actor {
                Actor::User(u) => Some(u.0.0.0),
                Actor::App { .. } | Actor::Anonymous => None,
            },
            scopes: p.scopes.0,
            token: Digest(token),
            sso: p.sso_orgs.iter().map(|o| o.0.0.0).collect(),
        }
    }

    fn principal(self) -> Principal {
        Principal {
            actor: self.account.map_or(Actor::Anonymous, |a| Actor::User(UserId(AccountId(DbId(a))))),
            scopes: ScopeSet(self.scopes),
            credential: Credential::from_wire(self.token.0, self.session),
            sso_orgs: self.sso.into_iter().map(|o| OrgId(AccountId(DbId(o)))).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PlanWire {
    base_old: ObjectId,
    head: ObjectId,
    commit: ObjectId,
    pack: PackRef,
    method: MergeMethod,
    up_to_date: bool,
    verified: TreeDiffDigest,
    engine: EngineVersion,
}

impl PlanWire {
    fn of(p: &MergePlan) -> PlanWire {
        PlanWire {
            base_old: p.base_old,
            head: p.head,
            commit: p.commit,
            pack: p.pack.pack().clone(),
            method: p.method,
            up_to_date: p.up_to_date,
            verified: p.verified,
            engine: p.engine.clone(),
        }
    }

    fn plan(self) -> MergePlan {
        MergePlan {
            base_old: self.base_old,
            head: self.head,
            commit: self.commit,
            pack: DurablePack::trusted(self.pack),
            method: self.method,
            up_to_date: self.up_to_date,
            verified: self.verified,
            engine: self.engine,
        }
    }
}

#[allow(clippy::large_enum_variant)] // one per command, built and dropped at the binding
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CmdWire {
    Merge { n: IssueNumber, plan: PlanWire, expected_head: Option<ObjectId>, actor: PrincipalWire },
    Enqueue { n: IssueNumber, head: ObjectId, actor: Option<PrincipalWire> },
    Dequeue { n: IssueNumber, actor: Option<PrincipalWire> },
    Build { base: RefName, entries: SmallVec<[MergeQueueEntry; 8]>, pack: PackRef, actor: Option<PrincipalWire> },
    Eject { n: IssueNumber, reason: EjectReason, actor: Option<PrincipalWire> },
    Land { plans: Vec<(IssueNumber, PlanWire)>, entry: MergeQueueEntry },
    Cursor { through: Index },
}

#[derive(Serialize, Deserialize)]
struct Wire {
    format: (u16, u16),
    cmd: CmdWire,
    #[serde(default)]
    render: Value,
}

impl CmdWire {
    fn of(c: &MergeCmd) -> CmdWire {
        let who = |a: &Option<Principal>| a.as_ref().map(PrincipalWire::of);
        match c {
            MergeCmd::Merge(m) => CmdWire::Merge {
                n: m.n,
                plan: PlanWire::of(&m.plan),
                expected_head: m.expected_head,
                actor: PrincipalWire::of(&m.actor),
            },
            MergeCmd::Queue(q) => match &q.op {
                QueueAction::Enqueue { n, head } => CmdWire::Enqueue { n: *n, head: *head, actor: who(&q.actor) },
                QueueAction::Dequeue { n } => CmdWire::Dequeue { n: *n, actor: who(&q.actor) },
                QueueAction::Build { base, entries, pack } => CmdWire::Build {
                    base: base.clone(),
                    entries: entries.clone(),
                    pack: pack.pack().clone(),
                    actor: who(&q.actor),
                },
                QueueAction::Eject { n, reason } => {
                    CmdWire::Eject { n: *n, reason: reason.clone(), actor: who(&q.actor) }
                }
            },
            MergeCmd::Land(l) => {
                CmdWire::Land { plans: l.plans.iter().map(|(n, p)| (*n, PlanWire::of(p))).collect(), entry: l.entry }
            }
            MergeCmd::Cursor(t) => CmdWire::Cursor { through: *t },
        }
    }

    fn cmd(self) -> MergeCmd {
        let op = |op, actor: Option<PrincipalWire>| {
            MergeCmd::Queue(QueueOp { op, actor: actor.map(PrincipalWire::principal) })
        };
        match self {
            CmdWire::Merge { n, plan, expected_head, actor } => {
                MergeCmd::Merge(Merge { n, plan: plan.plan(), expected_head, actor: actor.principal() })
            }
            CmdWire::Enqueue { n, head, actor } => op(QueueAction::Enqueue { n, head }, actor),
            CmdWire::Dequeue { n, actor } => op(QueueAction::Dequeue { n }, actor),
            CmdWire::Build { base, entries, pack, actor } => {
                op(QueueAction::Build { base, entries, pack: DurablePack::trusted(pack) }, actor)
            }
            CmdWire::Eject { n, reason, actor } => op(QueueAction::Eject { n, reason }, actor),
            CmdWire::Land { plans, entry } => {
                MergeCmd::Land(QueueMerge { plans: plans.into_iter().map(|(n, p)| (n, p.plan())).collect(), entry })
            }
            CmdWire::Cursor { through } => MergeCmd::Cursor(through),
        }
    }
}

impl Serialize for MergeCall {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Wire {
            format: (REPO_FORMAT_V4.family, REPO_FORMAT_V4.version),
            cmd: CmdWire::of(&self.cmd),
            render: self.render.clone(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for MergeCall {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<MergeCall, D::Error> {
        let v = Value::deserialize(d)?;
        let f = v.get("format").cloned().unwrap_or(Value::Null);
        if f != serde_json::json!([REPO_FORMAT_V4.family, REPO_FORMAT_V4.version]) {
            return Err(serde::de::Error::custom(format!("merge command format {f} is not one this release reads")));
        }
        let w: Wire = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
        Ok(MergeCall { cmd: w.cmd.cmd(), render: w.render })
    }
}

/// A refused merge command as a member answers it: the pull request it names and why, which the `MergeQueue`
/// consumer reads back for `on_refused`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeRefusal {
    pub n: IssueNumber,
    pub err: MergeError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::RepoCmd;
    use graftyard_raft::Command;

    fn oid(b: u8) -> ObjectId {
        ObjectId([b; 20])
    }

    fn plan() -> MergePlan {
        MergePlan {
            base_old: oid(1),
            head: oid(2),
            commit: oid(3),
            pack: DurablePack::trusted(PackRef::Inline(vec![1, 2, 3])),
            method: MergeMethod::Squash,
            up_to_date: false,
            verified: TreeDiffDigest(Digest([4; 32])),
            engine: EngineVersion("git version 2.56.0".into()),
        }
    }

    fn who() -> Principal {
        Principal {
            actor: Actor::User(UserId(AccountId(DbId(7)))),
            scopes: ScopeSet(1),
            credential: Credential::Stored([9; 32]),
            sso_orgs: SmallVec::new(),
        }
    }

    #[test]
    fn every_merge_command_round_trips_through_its_wire_form_and_names_32_4() {
        let entry = MergeQueueEntry { pr: IssueNumber(4), group_tip: oid(3), parent_tip: oid(1) };
        let main = RefName::parse(b"refs/heads/main").unwrap();
        let cmds = [
            MergeCmd::Merge(Merge { n: IssueNumber(4), plan: plan(), expected_head: Some(oid(2)), actor: who() }),
            MergeCmd::Queue(QueueOp {
                op: QueueAction::Enqueue { n: IssueNumber(4), head: oid(2) },
                actor: Some(who()),
            }),
            MergeCmd::Queue(QueueOp { op: QueueAction::Dequeue { n: IssueNumber(4) }, actor: Some(who()) }),
            MergeCmd::Queue(QueueOp {
                op: QueueAction::Build {
                    base: main,
                    entries: smallvec::smallvec![entry],
                    pack: DurablePack::trusted(PackRef::Inline(vec![5])),
                },
                actor: None,
            }),
            MergeCmd::Queue(QueueOp {
                op: QueueAction::Eject { n: IssueNumber(4), reason: EjectReason::CheckFailed { name: "ci".into() } },
                actor: None,
            }),
            MergeCmd::Land(QueueMerge { plans: vec![(IssueNumber(4), plan())], entry }),
            MergeCmd::Cursor(Index(12)),
        ];
        for cmd in cmds {
            let call = MergeCall { cmd: cmd.clone(), render: serde_json::json!({ "what": 1 }) };
            let v = serde_json::to_value(RepoCmd::Merge(Box::new(call.clone()))).unwrap();
            assert_eq!(v["merge"]["format"], serde_json::json!([0x20, 4]));
            let back: RepoCmd = serde_json::from_value(v).unwrap();
            assert_eq!(back, RepoCmd::Merge(Box::new(call)));
            assert_eq!(back.format(), REPO_FORMAT_V4);
        }
    }

    #[test]
    fn a_merge_command_in_a_later_format_is_refused_before_its_body_is_read() {
        let call = MergeCall { cmd: MergeCmd::Cursor(Index(1)), render: Value::Null };
        let mut v = serde_json::to_value(RepoCmd::Merge(Box::new(call))).unwrap();
        v["merge"]["format"] = serde_json::json!([0x20, 6]);
        v["merge"]["cmd"] = serde_json::json!("a field a later release added");
        let e = serde_json::from_value::<RepoCmd>(v).unwrap_err().to_string();
        assert!(e.contains("not one this release reads"), "{e}");
    }
}
