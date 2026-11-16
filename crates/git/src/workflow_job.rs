//! One Actions job's `workflow_job` steps on its repository chain (`design/SHARED.md`, "Multi-chain operations",
//! Workflow run, and "Event fan-out", App deliveries; `decisions/OPEN-QUESTIONS.md`, R24).
//!
//! - **The event.** `RepoEvent::WorkflowJob` (tag 46) is one step of one job: `queued` when the job enters `Queued`
//!   (at run creation, or at `NeedsMet` for a job with `needs`), `in_progress` at `Acquired`, `completed` at its
//!   conclusion. `RunOwner` appends them with its `RunChecksCmd`, from the shard's entries, so a step that passed
//!   between two of its rounds still gets its event.
//! - **The row.** The metadata `workflow_job` row holds the job's last step. `RunChecksCmd::decide` drops a step at or
//!   below it, so a replay after a crash, or a job queued again after a lease lost before `acquirejob`, writes
//!   nothing twice. The row is pruned with the check rows (`RepoSettings.check_retention_days`).
//! - **Format 32.24** (`REPO_FORMAT_WORKFLOW_JOB`): a `RunChecksCmd` that carries steps, and the event. A member reads
//!   it only after a site day onto a release that lists it, and `RunOwner` sends none before the site variable
//!   `GRAFTYARD_APP_HOOKS` is `1`, which a site day sets once `FormatsEnable` enabled 32.24.

use graftyard_raft::Format;
use graftyard_types::{DbId, Timestamp};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issues::CheckConclusion;
use crate::refs::ObjectId;

pub const REPO_FORMAT_WORKFLOW_JOB: Format = Format { family: 0x20, version: 24 };

/// GHES's `workflow_job` actions this install sends. `waiting` (an environment's protection rule) has no source yet.
/// Wire tags: add variants at the end only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum JobAction {
    Queued,
    InProgress,
    Completed,
}

impl JobAction {
    pub fn name(self) -> &'static str {
        match self {
            JobAction::Queued => "queued",
            JobAction::InProgress => "in_progress",
            JobAction::Completed => "completed",
        }
    }
}

/// One step of a job, with what GHES's `workflow_job` object needs that the check run lacks. `job` is the job's id,
/// which is also its check run's; `run` is the workflow run's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJobUpdate {
    pub job: DbId,
    pub run: DbId,
    pub attempt: u16,
    pub action: JobAction,
    pub name: Box<str>,
    /// The workflow file's path, as `WorkflowRun.workflow`.
    pub workflow: Box<str>,
    pub head_sha: ObjectId,
    pub head_ref: Box<str>,
    pub labels: SmallVec<[Box<str>; 4]>,
    /// The runner that took the job, for `in_progress` and `completed`.
    pub runner: Option<DbId>,
    pub conclusion: Option<CheckConclusion>,
    /// When the job was first queued, the job took its runner, and it ended.
    pub created: Timestamp,
    pub started: Option<Timestamp>,
    pub completed: Option<Timestamp>,
}
