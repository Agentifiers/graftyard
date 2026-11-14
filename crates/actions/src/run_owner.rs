//! `RunOwner`'s pure half (`design/SHARED.md`, "Multi-chain operations", Workflow run; BF-56): what one round writes,
//! from an Actions shard's fold and the committed entries after its cursor.
//!
//! A round takes the runs the entries touched and builds each run's check suite and check runs from the fold as it
//! stands, not from the entries, so a replay after a crash sends the same rows or newer ones and `RunChecksCmd::decide`
//! drops what the repository already holds. The host commits every repository's `RunChecksCmd` first, then the
//! shard's half: `SetRunStatus` for every touched run and last `OwnerCursor`. A `Gone` answer adds the repository to
//! `gone`, and the host builds the round again, which sends that repository nothing and commits `CancelRun` for its
//! unfinished runs. So the cursor passes a `Completed` job only once the repository took or refused its conclusion.
//! Dispatch never waits on the repository: a job's `needs` are released by the shard's own commands (`crate::shard`).

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::issues::{CheckConclusion, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId, CheckSuiteUpdate};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::run_checks::{ACTIONS_APP, RunChecksCmd};
use graftyard_types::{ChainId, Index, RepoId};

use crate::ids::RunId;
use crate::job::{Conclusion, JobState, RunStatus};
use crate::shard::{ActionsEvent, ActionsFold, ActionsOp, RunRow, consumer_request};

/// One round's writes. `checks` go first; `then` only after every one of them is acknowledged or refused `Gone`.
#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    pub checks: Vec<(RepoId, RunChecksCmd)>,
    pub then: Vec<ActionsOp>,
}

/// The runs that the entries after the cursor touched. `OwnerCursor` and `RunStatus` are `RunOwner`'s own writes,
/// which touch nothing, so a round's own entry starts no further round.
pub fn touched<'a>(fold: &ActionsFold, entries: impl IntoIterator<Item = &'a ActionsEvent>) -> BTreeSet<RunId> {
    let mut out = BTreeSet::new();
    for ev in entries {
        match ev {
            ActionsEvent::RunCreated { run, .. } => {
                out.insert(run.id);
            }
            ActionsEvent::Job { job, .. } => {
                if let Some(j) = fold.job(*job) {
                    out.insert(j.run);
                }
            }
            _ => {}
        }
    }
    out
}

/// The round for `runs` on shard `chain`, through entry `through`. `gone` names repositories that refused an earlier
/// round's checks: their unfinished runs are cancelled, and their checks are not sent again.
/// With no run touched the round is empty and the cursor stays: it lags over idle entries, and the next round with
/// work moves it, so `RunOwner`'s own cursor entries never start another round.
pub fn round(
    chain: ChainId,
    fold: &ActionsFold,
    runs: &BTreeSet<RunId>,
    through: Index,
    gone: &BTreeSet<RepoId>,
) -> Round {
    if runs.is_empty() {
        return Round { checks: Vec::new(), then: Vec::new() };
    }
    let mut by_repo: BTreeMap<RepoId, (Vec<CheckSuiteUpdate>, Vec<CheckRunUpdate>)> = BTreeMap::new();
    let mut then = Vec::new();
    for r in runs {
        let Some(row) = fold.run(*r) else { continue };
        if gone.contains(&row.run.repo) {
            if !matches!(row.status, RunStatus::Completed(_)) {
                then.push(ActionsOp::CancelRun { run: *r });
                then.push(ActionsOp::SetRunStatus { run: *r });
            }
            continue;
        }
        let (suites, checks) = by_repo.entry(row.run.repo).or_default();
        suites.push(suite(row));
        checks.extend(fold.run_jobs(*r).into_iter().map(|j| check_run(row, &j.spec.name, j.spec.id.0, &j.state)));
        then.push(ActionsOp::SetRunStatus { run: *r });
    }
    then.push(ActionsOp::OwnerCursor { through });
    // One command per repository and suite, so a command stays one run's rows: a 256-job matrix is one entry.
    let mut out = Vec::new();
    for (repo, (suites, checks)) in by_repo {
        for s in suites {
            let runs: Vec<CheckRunUpdate> = checks.iter().filter(|c| c.suite == s.suite).cloned().collect();
            let mut key = Vec::new();
            graftyard_fold::canon::write(&mut key, &(repo, &s, &runs));
            let request = consumer_request(chain, &key);
            out.push((repo, RunChecksCmd { suite: Some(s), runs, request }));
        }
    }
    Round { checks: out, then }
}

/// A row's version follows its state, so a replay never moves it back: 1 queued, 2 in progress, 3 completed.
fn version(status: CheckStatus) -> u64 {
    match status {
        CheckStatus::Completed => 3,
        CheckStatus::InProgress => 2,
        _ => 1,
    }
}

pub fn conclusion(c: Conclusion) -> CheckConclusion {
    match c {
        Conclusion::Success => CheckConclusion::Success,
        Conclusion::Failure => CheckConclusion::Failure,
        Conclusion::Cancelled => CheckConclusion::Cancelled,
        Conclusion::Skipped => CheckConclusion::Skipped,
        Conclusion::TimedOut => CheckConclusion::TimedOut,
        Conclusion::StartupFailure => CheckConclusion::StartupFailure,
    }
}

fn suite(row: &RunRow) -> CheckSuiteUpdate {
    let (status, c) = match row.status {
        RunStatus::Queued => (CheckStatus::Queued, None),
        RunStatus::InProgress => (CheckStatus::InProgress, None),
        RunStatus::Completed(c) => (CheckStatus::Completed, Some(conclusion(c))),
    };
    CheckSuiteUpdate {
        suite: CheckSuiteId(row.run.id.0),
        app: ACTIONS_APP,
        head_sha: ObjectId(row.run.head_sha),
        head_branch: RefName::parse(row.run.head_ref.as_bytes()).ok(),
        version: version(status),
        status,
        conclusion: c,
    }
}

/// A job's check run: its id is the job's, so one (job, attempt) is always one row.
fn check_run(row: &RunRow, name: &str, job: graftyard_types::DbId, s: &JobState) -> CheckRunUpdate {
    let (status, c, completed) = match s {
        JobState::Pending { .. } | JobState::Queued { .. } => (CheckStatus::Queued, None, None),
        JobState::Assigned { .. } | JobState::InProgress { .. } => (CheckStatus::InProgress, None, None),
        JobState::Completed { conclusion: k, at } => (CheckStatus::Completed, Some(conclusion(*k)), Some(*at)),
    };
    CheckRunUpdate {
        run: CheckRunId(job),
        suite: CheckSuiteId(row.run.id.0),
        app: ACTIONS_APP,
        head_sha: ObjectId(row.run.head_sha),
        name: name.into(),
        version: version(status),
        status,
        conclusion: c,
        started: None,
        completed,
        external_id: None,
        details_url: None,
        output: None,
    }
}
