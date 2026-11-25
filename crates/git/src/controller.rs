//! The controller's own task model (`design/AGENT-LAYER.md`, Shape, "Controller": `Task`, `TaskState`, `TaskBudget`,
//! `Workspace`; journey A3; AM3-WP1), pure. The controller holds a `Task` per Graftyard-run task in the agents
//! account; it is no group's fold. The base's record of the same task is the `AgentTask` row (`crate::task`), which the
//! controller moves only through the public task API, so this module also says which `PATCH` each `TaskState` sends.
//!
//! - **A new task** (`Task::new`) is `Queued`, on a `SessionId::Run`, with its refs under the host's prefix
//!   (`task::run_prefix`) and its `Workspace` from the org's `agents_workspace`: `Fork(<org>-agents/<repo>)` or
//!   `InRepo` (`workspace`).
//! - **A lost Sandbox** (`TaskEvent::Lost`) moves a running task onto a new Sandbox and keeps `last_push`, so the new
//!   one starts from that push, or from the base with none (`Task::resume`): at most the work since the last push is
//!   lost (`design/AGENT-LAYER.md`, Failure modes, controller instance).
//! - **Steps** (`Task::step`): a Sandbox starts it, pushes record `last_push`, a draft PR or a clean end finishes it, a
//!   failure, a budget over any limit (`TaskBudget::over`), or the cancel delivery ends it. A title or body edit by a
//!   non-writer after the snapshot pauses it, as the host's pause does (`waiting_for_user`), and only a writer's
//!   reassignment queues it again, with its snapshot moved to that entry's index. A final task (`Draft`, `Done`,
//!   `Failed`, `Cancelled`) takes no step.
//! - **The base's state** (`TaskState::api`): the `PATCH .../tasks/{id}` state each controller state sends, by the
//!   table in "Tasks and the activity feed". `Cancelled` sends none, because only the cancel endpoint sets it and
//!   `PATCH` may never (`TaskErr::HostState`).

use std::time::Duration;

use serde::{Deserialize, Serialize};

use graftyard_identity::{Login, RepoName, SessionId, UserId, WorkspaceMode};
use graftyard_types::Index;

use crate::agent::RefPattern;
use crate::issues::IssueNumber;
use crate::refs::ObjectId;
use crate::task::{TaskApiState, run_prefix};

/// A repository by owner and name, as the controller addresses it over REST.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: Login,
    pub name: RepoName,
}

/// An issue of the task's base repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueRef {
    pub repo: RepoRef,
    pub number: IssueNumber,
}

/// The Sandbox a running task holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxId(pub Box<str>);

/// Where the task pushes: the repository's fork in the org's `<org>-agents` workspace org, or the base itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Workspace {
    Fork(RepoRef),
    InRepo,
}

/// Why no `Workspace` exists for a base: its owner's login plus `-agents` is over the 39-character login limit, so
/// the workspace org cannot exist (`decisions/OPEN-QUESTIONS.md`, A51).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoWorkspaceOrg;

/// The task's `Workspace` under the org's `agents_workspace`: `Fork(<org>-agents/<repo>)`, the fork keeping the base's
/// name, or `InRepo`.
pub fn workspace(mode: WorkspaceMode, base: &RepoRef) -> Result<Workspace, NoWorkspaceOrg> {
    match mode {
        WorkspaceMode::InRepo => Ok(Workspace::InRepo),
        WorkspaceMode::Fork => {
            let owner = Login::parse(&format!("{}-agents", base.owner.as_str())).map_err(|_| NoWorkspaceOrg)?;
            Ok(Workspace::Fork(RepoRef { owner, name: base.name.clone() }))
        }
    }
}

/// The default wall-clock cap of a task (`design/AGENT-LAYER.md`, Shape, `TaskBudget`).
pub const TASK_WALL: Duration = Duration::from_secs(60 * 60);

/// Counted from each model response's usage. Over any limit the task ends as `Failed { reason }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBudget {
    pub task_tokens: u64,
    pub org_month_tokens: u64,
    pub wall: Duration,
}

/// What a task has used so far: its own model tokens, its org's this month with it, and its time since it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub task_tokens: u64,
    pub org_month_tokens: u64,
    pub elapsed: Duration,
}

impl TaskBudget {
    /// The limit `used` is over, as the `Failed` reason: a token count past its limit, or the wall reached.
    pub fn over(&self, used: Usage) -> Option<&'static str> {
        if used.task_tokens > self.task_tokens {
            Some("the task's token budget is used up")
        } else if used.org_month_tokens > self.org_month_tokens {
            Some("the org's monthly token budget is used up")
        } else if used.elapsed >= self.wall {
            Some("the task reached its wall-clock cap")
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Queued,
    Running { sandbox: SandboxId, last_push: Option<ObjectId> },
    Paused { reason: Box<str> },
    Draft { pr: IssueNumber },
    Done,
    Failed { reason: Box<str> },
    Cancelled,
}

impl TaskState {
    pub fn is_final(&self) -> bool {
        matches!(self, TaskState::Draft { .. } | TaskState::Done | TaskState::Failed { .. } | TaskState::Cancelled)
    }

    /// The state the controller sends to the base with `PATCH .../tasks/{id}`, or `None` for `Cancelled`, which only
    /// the cancel endpoint sets.
    pub fn api(&self) -> Option<TaskApiState> {
        Some(match self {
            TaskState::Queued => TaskApiState::Queued,
            TaskState::Running { .. } => TaskApiState::InProgress,
            TaskState::Paused { .. } => TaskApiState::WaitingForUser,
            TaskState::Draft { .. } | TaskState::Done => TaskApiState::Completed,
            TaskState::Failed { .. } => TaskApiState::Failed,
            TaskState::Cancelled => return None,
        })
    }
}

/// What moves a task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    /// A Sandbox started for a queued task.
    Started {
        sandbox: SandboxId,
    },
    /// The Sandbox `lost` stopped or was killed mid-task, and `sandbox` started in its place.
    Lost {
        lost: SandboxId,
        sandbox: SandboxId,
    },
    /// The agent pushed under the task's prefix.
    Pushed {
        head: ObjectId,
    },
    /// The model usage after a response.
    Used(Usage),
    /// The issue's title or body changed after the task's snapshot, by a writer or not.
    IssueEdited {
        by_writer: bool,
    },
    /// The issue was assigned to the controller's bot again, by a writer or not, in the entry at `at`.
    Reassigned {
        by_writer: bool,
        at: Index,
    },
    /// The draft PR is open and the agent is finished.
    Drafted {
        pr: IssueNumber,
    },
    /// Finished with nothing to open.
    Finished,
    Failed {
        reason: Box<str>,
    },
    /// The first-party delivery's `cancelled`.
    Cancelled,
}

/// A `graftyard_task` delivery's `action`: a new task to start, or a cancel to stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    Created,
    Cancelled,
}

/// Where a running task's Sandbox starts its work: the task's last push, or the base when it has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resume {
    Base,
    Push(ObjectId),
}

/// Why a step is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepErr {
    /// The task is final.
    Final,
    /// The event does not apply in the task's state.
    NotNow,
}

/// The controller's Graftyard-run task. `snapshot` is `AgentTask.snapshot`, the issue as of the task's create index,
/// or none for a prompt-only task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: SessionId,
    pub issue: Option<IssueRef>,
    pub snapshot: Option<Index>,
    pub requester: UserId,
    pub base: RepoRef,
    pub workspace: Workspace,
    /// `refs/heads/graftyard/<id>/**` (`task::run_prefix`).
    pub branch: RefPattern,
    pub budget: TaskBudget,
    pub state: TaskState,
}

impl Task {
    /// A queued task for a Graftyard-run session; `None` for an external session, or when the org's workspace cannot
    /// exist (`workspace`).
    pub fn new(
        id: SessionId,
        issue: Option<IssueRef>,
        snapshot: Option<Index>,
        requester: UserId,
        base: RepoRef,
        mode: WorkspaceMode,
        budget: TaskBudget,
    ) -> Option<Task> {
        let branch = run_prefix(&id)?;
        let workspace = workspace(mode, &base).ok()?;
        Some(Task { id, issue, snapshot, requester, base, workspace, branch, budget, state: TaskState::Queued })
    }

    /// The task a first-party `graftyard_task` delivery names (`design/AGENT-LAYER.md`, "API reference"): its
    /// `action`, and the queued `Task` for a `created` one, from the payload's `task` and `repository`. Only a
    /// Graftyard-run task in `queued` with a requester is one; an in-repo task works in its base, and a fork-mode task
    /// is AM3-WP3's rest, so `mode` comes from the org's settings and `None` for a fork without its workspace org.
    pub fn of_delivery(
        payload: &serde_json::Value,
        mode: WorkspaceMode,
        budget: TaskBudget,
    ) -> Option<(Delivery, Task)> {
        let t = &payload["task"];
        if t["kind"] != "graftyard_run" {
            return None;
        }
        let id = match SessionId::parse(t["id"].as_str()?, None)? {
            s @ SessionId::Run(_) => s,
            SessionId::External { .. } => return None,
        };
        let (owner, name) = payload["repository"]["full_name"].as_str()?.split_once('/')?;
        let base = RepoRef { owner: Login::parse(owner).ok()?, name: RepoName::parse(name).ok()? };
        let requester = UserId(graftyard_types::AccountId(graftyard_types::DbId(t["requester"]["id"].as_i64()?)));
        let issue = t["issue_number"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| IssueRef { repo: base.clone(), number: IssueNumber(n) });
        let action = match payload["action"].as_str()? {
            "created" if t["state"] == "queued" => Delivery::Created,
            "cancelled" => Delivery::Cancelled,
            _ => return None,
        };
        // The snapshot is the create's index, which the payload does not carry: the controller reads the issue as of
        // its task's `snapshot` over REST (journey A3 step 4), so a prompt-only task has none.
        let task = Task::new(id, issue, None, requester, base, mode, budget)?;
        Some((action, task))
    }

    /// Where the task's current Sandbox starts: `None` unless the task is running.
    pub fn resume(&self) -> Option<Resume> {
        match &self.state {
            TaskState::Running { last_push: Some(h), .. } => Some(Resume::Push(*h)),
            TaskState::Running { last_push: None, .. } => Some(Resume::Base),
            _ => None,
        }
    }

    /// Moves the task by `ev`. Refused on a final task, and for an event its state does not take; a refused step
    /// changes nothing.
    pub fn step(&mut self, ev: TaskEvent) -> Result<(), StepErr> {
        use TaskState as S;
        if self.state.is_final() {
            return Err(StepErr::Final);
        }
        let paused = || S::Paused { reason: "the issue was edited by someone who cannot push".into() };
        self.state = match (&self.state, ev) {
            (_, TaskEvent::Cancelled) => S::Cancelled,
            (_, TaskEvent::Failed { reason }) => S::Failed { reason },
            (_, TaskEvent::Used(u)) => match self.budget.over(u) {
                Some(why) => S::Failed { reason: why.into() },
                None => return Ok(()),
            },
            (S::Queued | S::Running { .. }, TaskEvent::IssueEdited { by_writer: false }) => paused(),
            (_, TaskEvent::IssueEdited { .. }) => return Ok(()),
            (S::Paused { .. }, TaskEvent::Reassigned { by_writer: true, at }) => {
                self.snapshot = Some(at);
                S::Queued
            }
            (_, TaskEvent::Reassigned { .. }) => return Ok(()),
            (S::Queued, TaskEvent::Started { sandbox }) => S::Running { sandbox, last_push: None },
            (S::Running { sandbox, last_push }, TaskEvent::Lost { lost, sandbox: new })
                if *sandbox == lost && new != lost =>
            {
                S::Running { sandbox: new, last_push: *last_push }
            }
            (S::Running { sandbox, .. }, TaskEvent::Pushed { head }) => {
                S::Running { sandbox: sandbox.clone(), last_push: Some(head) }
            }
            (S::Running { .. }, TaskEvent::Drafted { pr }) => S::Draft { pr },
            (S::Running { .. }, TaskEvent::Finished) => S::Done,
            _ => return Err(StepErr::NotNow),
        };
        Ok(())
    }
}
