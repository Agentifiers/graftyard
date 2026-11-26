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
//!
//! R24: `steps` reads each job's `workflow_job` steps from the entries themselves, not the fold, so a job queued and
//! taken between two rounds still gets both. The round puts them in its `RunChecksCmd` (format 32.24).
//!
//! `gh-token`: a round also denies the `GITHUB_TOKEN` of every job its entries ended (`crate::token::denies`), as
//! `TokensOp::RevokeJti` on each `jti`'s shard, beside the repository writes and before the cursor, so a crash after
//! the denies and before the cursor sends them again and a deny is never skipped. A round with denies and no touched
//! run still moves the cursor.

use std::collections::{BTreeMap, BTreeSet};

use graftyard_git::issues::{
    CheckAnnotation, CheckConclusion, CheckOutput, CheckRunId, CheckRunUpdate, CheckStatus, CheckSuiteId,
    CheckSuiteUpdate,
};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::run_checks::{ACTIONS_APP, RunChecksCmd, RunChecksOut};
use graftyard_git::workflow_job::{JobAction, WorkflowJobUpdate};
use graftyard_identity::{Jti, TokensCmd, TokensOp};
use graftyard_types::{AccountId, ChainId, CredentialShard, DbId, Index, RepoId, Timestamp};

use crate::ids::RunId;
use crate::job::{Conclusion, JobEvent, JobState, RunStatus};
use crate::shard::{ActionsActor, ActionsCmd, ActionsEvent, ActionsFold, ActionsOp, RunRow, consumer_request};

/// One round's writes. `checks` and `denies` go first; `then` only after every check is acknowledged or refused
/// `Gone` and every deny is acknowledged.
#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    pub checks: Vec<(RepoId, RunChecksCmd)>,
    /// The job tokens to revoke: each `TokensOp::RevokeJti` under the operator's credential (`deny_command`).
    pub denies: Vec<TokensCmd>,
    pub then: Vec<ActionsOp>,
}

/// `TokensOp::RevokeJti` for a job's token, under the operator's credential, which the leader takes for any `jti`.
pub fn deny_command(jti: Jti, exp: Timestamp) -> TokensCmd {
    TokensCmd { cred: CredentialShard::NONE, op: TokensOp::RevokeJti { jti, exp } }
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

/// Each job's `workflow_job` steps in the committed `entries` after the cursor, each with its entry's time, in order.
/// An event at a job's version v made the job's version v+1, so when the fold is at v+1 the fold's state is what the
/// event did, which tells a lease lost after `acquirejob` (completed) from one lost before (queued again). An event
/// the fold has moved past did not complete the job, since a completed job takes no later event.
pub fn steps<'a>(
    fold: &ActionsFold,
    entries: impl IntoIterator<Item = (Timestamp, &'a ActionsEvent)>,
) -> Vec<WorkflowJobUpdate> {
    let mut out = Vec::new();
    let mut matched: BTreeMap<crate::JobId, crate::RunnerId> = BTreeMap::new();
    let step = |run: &RunRow, spec: &crate::shard::JobSpec, action, at: Timestamp| WorkflowJobUpdate {
        job: spec.id.0,
        run: run.run.id.0,
        attempt: run.run.attempt,
        action,
        name: spec.name.clone(),
        workflow: run.run.workflow.clone(),
        head_sha: ObjectId(run.run.head_sha),
        head_ref: run.run.head_ref.clone(),
        labels: spec.labels.iter().map(Into::into).collect(),
        runner: None,
        conclusion: None,
        created: at,
        started: None,
        completed: None,
    };
    for (at, ev) in entries {
        match ev {
            ActionsEvent::RunCreated { run, jobs } => {
                let Some(row) = fold.run(run.id) else { continue };
                for spec in jobs.iter().filter(|s| s.needs.is_empty()) {
                    out.push(step(row, spec, JobAction::Queued, at));
                }
            }
            ActionsEvent::Job { job, version, event } => {
                let (Some(j), Some(row)) = (fold.job(*job), fold.job(*job).and_then(|j| fold.run(j.run))) else {
                    continue;
                };
                let now = (j.version == version + 1).then_some(&j.state);
                if let JobEvent::Matched(r) = event {
                    matched.insert(*job, *r);
                }
                let action = match (now, event) {
                    (Some(JobState::Completed { .. }), _) => JobAction::Completed,
                    (_, JobEvent::NeedsMet | JobEvent::LeaseExpired) => JobAction::Queued,
                    (_, JobEvent::Acquired) => JobAction::InProgress,
                    _ => continue,
                };
                let mut s = step(row, &j.spec, action, at);
                match action {
                    JobAction::InProgress => {
                        s.started = Some(at);
                        s.runner = matched.get(job).copied().or(j.state.runner()).map(|r| r.0);
                    }
                    JobAction::Completed => {
                        if let Some(JobState::Completed { conclusion: c, at: t }) = now {
                            s.conclusion = Some(conclusion(*c));
                            s.completed = Some(*t);
                        }
                    }
                    JobAction::Queued => {}
                }
                out.push(s);
            }
            _ => {}
        }
    }
    out
}

/// The round for `runs` on shard `chain`, through entry `through`. `gone` names repositories that refused an earlier
/// round's checks: their unfinished runs are cancelled, and their checks are not sent again. `jobs` are the entries'
/// `steps`, sent with their run's checks; none when the install has not enabled 32.24. `denies` are the entries' ended
/// job tokens (`crate::token::denies`); none until a site day set `GRAFTYARD_JOB_TOKENS`.
/// With no run touched and nothing to deny the round is empty and the cursor stays: it lags over idle entries, and the
/// next round with work moves it, so `RunOwner`'s own cursor entries never start another round.
#[allow(clippy::too_many_arguments)]
pub fn round(
    chain: ChainId,
    fold: &ActionsFold,
    runs: &BTreeSet<RunId>,
    through: Index,
    gone: &BTreeSet<RepoId>,
    jobs: &[WorkflowJobUpdate],
    denies: &[(Jti, Timestamp)],
) -> Round {
    let denies: Vec<TokensCmd> = denies.iter().map(|(j, exp)| deny_command(*j, *exp)).collect();
    if runs.is_empty() && denies.is_empty() {
        return Round { checks: Vec::new(), denies, then: Vec::new() };
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
        checks.extend(
            fold.run_jobs(*r)
                .into_iter()
                .map(|j| check_run(row, &j.spec.name, j.spec.id, &j.state, fold.job_annotations(j.spec.id))),
        );
        then.push(ActionsOp::SetRunStatus { run: *r });
    }
    then.push(ActionsOp::OwnerCursor { through });
    // One command per repository and suite, so a command stays one run's rows: a 256-job matrix is one entry.
    let mut out = Vec::new();
    for (repo, (suites, checks)) in by_repo {
        for s in suites {
            let runs: Vec<CheckRunUpdate> = checks.iter().filter(|c| c.suite == s.suite).cloned().collect();
            let steps: Vec<WorkflowJobUpdate> = jobs.iter().filter(|j| j.run == DbId(s.suite.0.0)).cloned().collect();
            let mut key = Vec::new();
            if steps.is_empty() {
                graftyard_fold::canon::write(&mut key, &(repo, &s, &runs));
            } else {
                graftyard_fold::canon::write(&mut key, &(repo, &s, &runs, &steps));
            }
            let request = consumer_request(chain, &key);
            out.push((repo, RunChecksCmd { suite: Some(s), runs, request, jobs: steps }));
        }
    }
    Round { checks: out, denies, then }
}

/// A round's shard half as the host sends it: each command under the operator's credential and a request id from
/// its place in the round (`through`, index), so a half sent again after a crash or timeout commits nothing twice and
/// is answered from the rows of what already committed. The host admits them all in one turn, so admission cuts them
/// into as few entries as ENTRY_SOFT allows (`decisions/DECISIONS.md`, "`replay-wait`").
pub fn shard_half(
    chain: ChainId,
    through: Index,
    then: Vec<ActionsOp>,
) -> Vec<(graftyard_types::RequestId, ActionsCmd)> {
    then.into_iter()
        .enumerate()
        .map(|(i, op)| {
            let mut key = Vec::new();
            graftyard_fold::canon::write(&mut key, &(through.0, i as u32));
            (consumer_request(chain, &key), ActionsCmd { actor: ActionsActor::Operator, op })
        })
        .collect()
}

/// What a repository's answer to one of a round's `RunChecksCmd`s means for the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChecksAnswer {
    Applied,
    /// The repository takes no more checks: the round is built again with it in `gone`, which cancels its runs.
    Gone,
    /// The round runs again, and the cursor stays.
    Retry,
}

/// `out` is the leader's outcome, `None` when the commit failed: no group answered, no leader, or a refusal. `gone`
/// is asked only then: `graftyard_identity::AuthzFold::repo_gone`, which needs the failure to be no group and the
/// owner the repository's `RepoOwner` row names to hold it under no name. A deleted repository's group answers `Gone`
/// from its tombstone; a purged one has no group, and those two proofs stand in for it (`decisions/DECISIONS.md`,
/// "`repo-gone`"). Without them a failure retries, so a repository that is only down, or live in a transfer, holds
/// the cursor and loses no check.
pub fn checks_answer(out: Option<RunChecksOut>, gone: bool) -> ChecksAnswer {
    match out {
        Some(RunChecksOut::Applied(_)) => ChecksAnswer::Applied,
        Some(RunChecksOut::Gone) => ChecksAnswer::Gone,
        Some(RunChecksOut::Unavailable) => ChecksAnswer::Retry,
        None if gone => ChecksAnswer::Gone,
        None => ChecksAnswer::Retry,
    }
}

/// The owner whose placement record a failed `RunChecksCmd` is sent again under, or `None` to keep the answer: only
/// when the commit under the shard owner's record found no group (`no_group`) and the repository's `RepoOwner` row
/// names another owner now. A run's repository can leave the shard's owner by a transfer, and its group can then
/// move to the new owner's placement, which the shard owner's record does not name (`decisions/DECISIONS.md`,
/// "`repo-gone`").
pub fn record_owner(shard_owner: AccountId, no_group: bool, repo_owner: Option<AccountId>) -> Option<AccountId> {
    repo_owner.filter(|o| no_group && *o != shard_owner)
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
fn check_run(
    row: &RunRow,
    name: &str,
    job: crate::JobId,
    s: &JobState,
    annotations: &[CheckAnnotation],
) -> CheckRunUpdate {
    let (status, c, completed) = match s {
        JobState::Pending { .. } | JobState::Queued { .. } => (CheckStatus::Queued, None, None),
        JobState::Assigned { .. } | JobState::InProgress { .. } => (CheckStatus::InProgress, None, None),
        JobState::Completed { conclusion: k, at } => (CheckStatus::Completed, Some(conclusion(*k)), Some(*at)),
    };
    // Only a finished job holds annotations (`ActionsFold::check`), so only a completed check run carries an output.
    let output = (!annotations.is_empty()).then(|| CheckOutput {
        title: "".into(),
        summary: "".into(),
        text: None,
        annotations: annotations.to_vec(),
    });
    CheckRunUpdate {
        run: CheckRunId(job.0),
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
        output,
    }
}
