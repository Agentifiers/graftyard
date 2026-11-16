//! `RunOwner`'s writes to a repository chain (`design/SHARED.md`, "Multi-chain operations", Workflow run;
//! `build/BUILD-PLAN.md`, M4-WP3, BF-56).
//!
//! `RunOwner` reads each Actions shard's committed log and sends `RunChecksCmd` to the run's repository: the run's
//! check suite and one check run per job, in the state the job is in, with a `RequiredCheck` for each one a branch rule
//! requires on that SHA. A check run's id is its job's id and its suite's id is its run's id, so one (job, attempt) is
//! always the same row: a re-run's jobs have fresh ids (SHARED, Idempotent run creation). Its `version` follows the job,
//! 1 queued, 2 in progress, 3 completed, and `decide` drops every update its row already holds, so a replay after a
//! crash commits nothing twice and a late one never moves a row back.
//!
//! **Format 32.21** (`REPO_FORMAT_ACTIONS`): `RunChecksCmd`. It writes no new event. A member reads it only after a
//! site day onto a release that lists it. 32.20 is `REPO_FORMAT_SPLIT` on `b030-split`. A command that carries `jobs`,
//! each job's `workflow_job` steps, is format 32.24 (`crate::workflow_job`).

use graftyard_raft::{Format, Record, TierSet};
use graftyard_types::{DbId, RequestId};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::issue_ops::{RepoView, required};
use crate::issues::{AppId, CheckRunUpdate, CheckSource, CheckStatus, CheckSuiteUpdate};
use crate::repo::{RepoEvent, RepoScratch};
use crate::tiers::{Timed, get_rec, int, sha_key, workflow_job};
use crate::workflow_job::WorkflowJobUpdate;

pub const REPO_FORMAT_ACTIONS: Format = Format { family: 0x20, version: 21 };

/// The Actions App, whose check runs `RunOwner` writes and whose installation a job token acts as (`design/DESIGN.md`,
/// `ActorId`): id 2 of cell 0's block 0, which `Ids` never leases, beside the install account's id 1 [inference: an
/// install-wide App needs an id no account or App takes; block 0 is the only range nothing else assigns].
pub const ACTIONS_APP: AppId = AppId(DbId(2));

/// `{"runChecks": ..}`. It carries no account and no key, as every consumer command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunChecksCmd {
    pub suite: Option<CheckSuiteUpdate>,
    pub runs: Vec<CheckRunUpdate>,
    pub request: RequestId,
    /// R24's steps, in the order the shard committed them. Absent from a 32.21 command, whose bytes it keeps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<WorkflowJobUpdate>,
}

impl Record for RunChecksCmd {
    fn encode(&self, out: &mut Vec<u8>) {
        graftyard_fold::canon::write(out, self);
    }
}

/// What a `RunChecksCmd` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunChecksOut {
    /// The updates it wrote; 0 for a replay.
    Applied(u32),
    /// The repository was deleted: `RunOwner` cancels the run (SHARED, the abort path).
    Gone,
    /// The member's metadata tier is still loading: `RunOwner` retries.
    Unavailable,
}

impl RunChecksCmd {
    pub fn decide(&self, s: &RepoScratch<'_>) -> (SmallVec<[RepoEvent; 4]>, RunChecksOut) {
        if crate::tiers::deleted(s.core()).is_some() {
            return (SmallVec::new(), RunChecksOut::Gone);
        }
        let Some((core, meta)) = s.view() else { return (SmallVec::new(), RunChecksOut::Unavailable) };
        let v = RepoView::new(s.fold().tiers(), core, meta, s.at());
        let mut evs = SmallVec::new();
        if let Some(u) = &self.suite {
            let key = [sha_key(&u.head_sha), int(u.suite.0.0)];
            if get_rec::<Timed<CheckSuiteUpdate>>(meta, "check_suite", &key).is_none_or(|t| t.rec.version < u.version) {
                evs.push(RepoEvent::CheckSuite(u.clone()));
            }
        }
        for u in &self.runs {
            let key = [sha_key(&u.head_sha), int(u.run.0.0)];
            if get_rec::<Timed<CheckRunUpdate>>(meta, "check_run", &key).is_some_and(|t| t.rec.version >= u.version) {
                continue;
            }
            evs.push(RepoEvent::CheckRun(u.clone()));
            if required(&v, u.head_sha, &u.name, Some(u.app)) {
                let done = u.status == CheckStatus::Completed;
                evs.push(RepoEvent::RequiredCheck {
                    sha: u.head_sha,
                    name: u.name.clone(),
                    source: CheckSource::Run { app: u.app, run: u.run },
                    version: u.version,
                    conclusion: if done { u.conclusion } else { None },
                });
            }
        }
        // A step at or below the job's row is a replay, or a job queued again after a lease lost before `acquirejob`.
        // A later step keeps what an earlier one knew: when the job was created and started, and its runner.
        let mut last: Vec<WorkflowJobUpdate> = Vec::new();
        for u in &self.jobs {
            let held = last.iter().rev().find(|h| h.job == u.job).cloned().or_else(|| workflow_job(meta, u.job));
            if held.as_ref().is_some_and(|h| h.action >= u.action) {
                continue;
            }
            let u = match held {
                Some(h) => WorkflowJobUpdate {
                    created: h.created,
                    started: u.started.or(h.started),
                    runner: u.runner.or(h.runner),
                    ..u.clone()
                },
                None => u.clone(),
            };
            last.push(u.clone());
            evs.push(RepoEvent::WorkflowJob(u));
        }
        let n = evs.iter().filter(|e| !matches!(e, RepoEvent::RequiredCheck { .. })).count() as u32;
        (evs, RunChecksOut::Applied(n))
    }

    /// 32.24 when it carries steps, else 32.21.
    pub fn format(&self) -> Format {
        if self.jobs.is_empty() { REPO_FORMAT_ACTIONS } else { crate::workflow_job::REPO_FORMAT_WORKFLOW_JOB }
    }

    pub fn replay(&self, events: &[RepoEvent]) -> RunChecksOut {
        RunChecksOut::Applied(events.iter().filter(|e| !matches!(e, RepoEvent::RequiredCheck { .. })).count() as u32)
    }

    /// `RequiredCheck` is core; check runs and suites are metadata.
    pub fn tiers(&self) -> TierSet {
        TierSet::CORE.union(TierSet::METADATA)
    }
}
