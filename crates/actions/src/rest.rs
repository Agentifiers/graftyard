//! `rest:actions` reads for a run and its jobs, as GHES 3.22 renders them: `workflow-run`, `job`, and the
//! `{total_count, jobs}` list (the pinned description, `web/src/api/ghes.d.ts`, `components.schemas`). Pure renderers
//! over the shard's rows: the edge reads the rows and the repository and passes what GHES shows of them.
//!
//! - **Ids.** A run's `run_number` is its id until `ActionsTrigger` numbers runs per workflow [inference: GHES
//!   numbers per workflow, which only a trigger that keeps a counter can do]. `workflow_id` is 0 for the same reason.
//! - **Times.** A job's `created_at` is the run's; `started_at` and `completed_at` come from the runner's own step
//!   report when the edge kept one (`steps`); `completed_at` is the job's own `at`, when the shard completed it.
//! - **Steps** are the runner's `WorkflowStepsUpdate` rows, merged by number, which the edge stores beside the job's
//!   logs; a job with none shows `steps: []`, as GHES shows a queued job.

use serde_json::{Value, json};

use crate::job::{Conclusion, JobState, RunStatus, derive_run_status};
use crate::shard::{JobRow, WorkflowRun};

/// What GHES shows of the repository around a run: its owner and name, its `minimal-repository` JSON, and the
/// install's web and API roots.
pub struct Ctx<'a> {
    pub web: &'a str,
    pub api: &'a str,
    pub full_name: &'a str,
    pub repository: &'a Value,
}

pub fn conclusion_word(c: Conclusion) -> &'static str {
    match c {
        Conclusion::Success => "success",
        Conclusion::Failure => "failure",
        Conclusion::Cancelled => "cancelled",
        Conclusion::Skipped => "skipped",
        Conclusion::TimedOut => "timed_out",
        Conclusion::StartupFailure => "startup_failure",
    }
}

/// A job's `status` and `conclusion`. `Pending` (needs unmet) is GHES's `waiting`; `Assigned` is still `queued`
/// until the runner acquires it. A `Held` job is listed by no read (`visible_jobs`), so its words are `Pending`'s. A
/// job waiting for `AgentGate` (`SecretsWait`, 0x15.14) is GHES's `waiting` too, as a job waiting on a protection rule
/// reads [doc: GHES 3.22 `job.status` enum: `queued`, `in_progress`, `completed`, `waiting`, `requested`, `pending`].
pub fn job_status(s: &JobState) -> (&'static str, Option<&'static str>) {
    match s {
        JobState::Pending { .. } | JobState::Held { .. } | JobState::SecretsWait { .. } => ("waiting", None),
        JobState::Queued { .. } | JobState::Assigned { .. } => ("queued", None),
        JobState::InProgress { .. } => ("in_progress", None),
        JobState::Completed { conclusion, .. } => ("completed", Some(conclusion_word(*conclusion))),
    }
}

fn run_status(s: RunStatus) -> (&'static str, Option<&'static str>) {
    match s {
        RunStatus::Queued => ("queued", None),
        RunStatus::InProgress => ("in_progress", None),
        RunStatus::Completed(c) => ("completed", Some(conclusion_word(c))),
        // `agent-ci`: a run awaiting a writer's approval, as GitHub renders a fork pull request's run awaiting
        // approval (`decisions/DECISIONS.md`, "`agent-ci`").
        RunStatus::Held => ("completed", Some("action_required")),
    }
}

/// The jobs GHES lists for a run: none of a run held for approval, whose jobs GitHub creates only once it is approved
/// (`decisions/DECISIONS.md`, "`agent-ci`").
pub fn visible_jobs(jobs: &[JobRow]) -> impl Iterator<Item = &JobRow> {
    jobs.iter().filter(|j| !matches!(j.state, JobState::Held { .. }))
}

fn hex(b: &[u8; 20]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn branch(r: &WorkflowRun) -> Option<&str> {
    r.head_ref.strip_prefix("refs/heads/")
}

/// GHES's `workflow-run`. `created` is when the run was created (the shard's first job `since`, or the caller's).
pub fn workflow_run(c: &Ctx<'_>, run: &WorkflowRun, jobs: &[JobRow], created: &str, updated: &str) -> Value {
    let states: Vec<JobState> = jobs.iter().map(|j| j.state.clone()).collect();
    let (status, conclusion) = run_status(derive_run_status(&states));
    let id = run.id.0.0;
    let base = format!("{}/repos/{}/actions/runs/{id}", c.api, c.full_name);
    json!({
        "id": id,
        "name": run.workflow.rsplit('/').next().unwrap_or(&run.workflow),
        "node_id": format!("WFR_{id}"),
        "head_branch": branch(run),
        "head_sha": hex(&run.head_sha),
        "path": &*run.workflow,
        "display_title": run.workflow.rsplit('/').next().unwrap_or(&run.workflow),
        "run_number": id,
        "run_attempt": run.attempt,
        "referenced_workflows": [],
        "event": &*run.event,
        "status": status,
        "conclusion": conclusion,
        "workflow_id": 0,
        "url": base,
        "html_url": format!("{}/{}/actions/runs/{id}", c.web, c.full_name),
        "pull_requests": [],
        "created_at": created,
        "updated_at": updated,
        "run_started_at": created,
        "jobs_url": format!("{base}/jobs"),
        "logs_url": format!("{base}/logs"),
        "check_suite_url": format!("{}/repos/{}/check-suites/{id}", c.api, c.full_name),
        "artifacts_url": format!("{base}/artifacts"),
        "cancel_url": format!("{base}/cancel"),
        "rerun_url": format!("{base}/rerun"),
        "previous_attempt_url": null,
        "workflow_url": format!("{}/repos/{}/actions/workflows/{id}", c.api, c.full_name),
        "head_commit": null,
        "repository": c.repository,
        "head_repository": c.repository,
    })
}

/// GHES's `job`. `steps` is the runner's last step report for it (`[{number, name, status, conclusion, started_at,
/// completed_at}]` in GHES's words), and `runner` the runner's id and name while one holds or held it.
pub fn job(
    c: &Ctx<'_>,
    run: &WorkflowRun,
    j: &JobRow,
    created: &str,
    steps: &[Value],
    runner: Option<(i64, &str)>,
) -> Value {
    let (status, conclusion) = job_status(&j.state);
    let id = j.spec.id.0.0;
    let first = steps.iter().find_map(|s| s["started_at"].as_str().map(str::to_owned));
    // The job's own end: a runner killed mid-step reports no end, and its last report is an earlier step's.
    let completed_at = match &j.state {
        JobState::Completed { at, .. } => Some(graftyard_wire::time::iso(*at)),
        _ => None,
    };
    let run_url = format!("{}/repos/{}/actions/runs/{}", c.api, c.full_name, run.id.0.0);
    json!({
        "id": id,
        "run_id": run.id.0.0,
        "run_url": run_url,
        "run_attempt": run.attempt,
        "node_id": format!("CR_{id}"),
        "head_sha": hex(&run.head_sha),
        "url": format!("{}/repos/{}/actions/jobs/{id}", c.api, c.full_name),
        "html_url": format!("{}/{}/actions/runs/{}/job/{id}", c.web, c.full_name, run.id.0.0),
        "status": status,
        "conclusion": conclusion,
        "created_at": created,
        "started_at": first.unwrap_or_else(|| created.to_owned()),
        "completed_at": completed_at,
        "name": &*j.spec.name,
        "steps": steps,
        "check_run_url": format!("{}/repos/{}/check-runs/{id}", c.api, c.full_name),
        "labels": j.spec.labels.iter().collect::<Vec<_>>(),
        "runner_id": runner.map(|r| r.0),
        "runner_name": runner.map(|r| r.1),
        "runner_group_id": runner.map(|_| 1),
        "runner_group_name": runner.map(|_| "Default"),
        "workflow_name": run.workflow.rsplit('/').next().unwrap_or(&run.workflow),
        "head_branch": branch(run),
    })
}

/// The runner's `WorkflowStepsUpdate` steps in GHES's words, merged into `kept` by number: its status codes are 3 (in
/// progress) and 6 (completed), its conclusions 2 (success), 3 (failure), 4 (cancelled), and 7 (skipped) [measured,
/// `tests/m4/wp3/runner-protocol.md`].
pub fn merge_steps(kept: &mut Vec<Value>, update: &Value) {
    for s in update["steps"].as_array().into_iter().flatten() {
        let Some(n) = s["number"].as_u64() else { continue };
        let status = match s["status"].as_u64() {
            Some(6) => "completed",
            Some(3) => "in_progress",
            _ => "queued",
        };
        let conclusion = match (status, s["conclusion"].as_u64()) {
            ("completed", Some(2)) => json!("success"),
            ("completed", Some(3)) => json!("failure"),
            ("completed", Some(4)) => json!("cancelled"),
            ("completed", Some(7)) => json!("skipped"),
            _ => Value::Null,
        };
        let row = json!({
            "name": s["name"], "number": n, "status": status, "conclusion": conclusion,
            "started_at": s["started_at"].as_str().map(trim_ms), "completed_at": s["completed_at"].as_str().map(trim_ms),
        });
        match kept.iter_mut().find(|k| k["number"].as_u64() == Some(n)) {
            Some(k) => *k = row,
            None => kept.push(row),
        }
    }
    kept.sort_by_key(|k| k["number"].as_u64());
}

/// The runner's millisecond timestamps to the second, as GitHub renders times (`graftyard_wire::time`).
fn trim_ms(t: &str) -> String {
    match t.split_once('.') {
        Some((s, _)) if t.ends_with('Z') => format!("{s}Z"),
        _ => t.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{JobId, RunId, RunnerId};
    use crate::labels::LabelSet;
    use crate::shard::JobSpec;
    use graftyard_types::{ChainId, DbId, Index, RepoId, Timestamp};

    /// GHES 3.22's required `workflow-run` and `job` fields (`web/src/api/ghes.d.ts`, the fields without `?`).
    const RUN_FIELDS: &[&str] = &[
        "id",
        "node_id",
        "head_branch",
        "head_sha",
        "path",
        "run_number",
        "event",
        "status",
        "conclusion",
        "workflow_id",
        "url",
        "html_url",
        "pull_requests",
        "created_at",
        "updated_at",
        "jobs_url",
        "logs_url",
        "check_suite_url",
        "artifacts_url",
        "cancel_url",
        "rerun_url",
        "workflow_url",
        "head_commit",
        "repository",
        "head_repository",
        "display_title",
    ];
    const JOB_FIELDS: &[&str] = &[
        "id",
        "run_id",
        "run_url",
        "node_id",
        "head_sha",
        "url",
        "html_url",
        "status",
        "conclusion",
        "created_at",
        "started_at",
        "completed_at",
        "name",
        "check_run_url",
        "labels",
        "runner_id",
        "runner_name",
        "runner_group_id",
        "runner_group_name",
        "workflow_name",
        "head_branch",
    ];

    fn row(id: i64, state: JobState) -> JobRow {
        JobRow {
            run: RunId(DbId(9)),
            spec: JobSpec {
                id: JobId(DbId(id)),
                name: "build".into(),
                labels: LabelSet::parse(["self-hosted", "linux"]).unwrap(),
                needs: Default::default(),
                matrix: Default::default(),
                environment: None,
                steps: "[]".into(),
            },
            state,
            version: 1,
        }
    }

    #[test]
    fn a_run_and_its_jobs_render_ghes_fields_and_states() {
        let run = WorkflowRun {
            id: RunId(DbId(9)),
            repo: RepoId(DbId(7)),
            workflow: ".github/workflows/ci.yml".into(),
            event: "workflow_dispatch".into(),
            head_sha: [0xab; 20],
            head_ref: "refs/heads/main".into(),
            trigger: (ChainId::Repo(RepoId(DbId(7))), Index(3)),
            attempt: 2,
            concurrency: None,
            fork: false,
        };
        let repo = json!({ "id": 7, "full_name": "o/r" });
        let c = Ctx { web: "https://h", api: "https://h/api/v3", full_name: "o/r", repository: &repo };
        let running = row(10, JobState::InProgress { runner: RunnerId(DbId(5)), lease_until: Timestamp(1) });
        let done = row(11, JobState::Completed { conclusion: Conclusion::Failure, at: Timestamp(1_763_100_000_000) });
        let r =
            workflow_run(&c, &run, &[running.clone(), done.clone()], "2026-11-14T00:00:00Z", "2026-11-14T00:01:00Z");
        for f in RUN_FIELDS {
            assert!(r.get(*f).is_some(), "workflow-run lacks {f}");
        }
        assert_eq!((r["status"].as_str(), r["conclusion"].as_str()), (Some("in_progress"), None));
        assert_eq!(r["head_branch"], "main");
        assert_eq!(r["jobs_url"], "https://h/api/v3/repos/o/r/actions/runs/9/jobs");

        // The runner's step report: one update starts step 2, the next completes it with failure and adds step 3.
        let mut steps = Vec::new();
        merge_steps(
            &mut steps,
            &json!({ "steps": [
            { "number": 2, "name": "Run make", "status": 3, "conclusion": 0, "started_at": "2026-11-14T00:00:05.120Z", "completed_at": null },
            { "number": 1, "name": "Set up job", "status": 6, "conclusion": 2, "started_at": "2026-11-14T00:00:04.000Z", "completed_at": "2026-11-14T00:00:05.000Z" } ]}),
        );
        merge_steps(
            &mut steps,
            &json!({ "steps": [
            { "number": 2, "name": "Run make", "status": 6, "conclusion": 3, "started_at": "2026-11-14T00:00:05.120Z", "completed_at": "2026-11-14T00:00:09.900Z" },
            { "number": 3, "name": "Complete job", "status": 6, "conclusion": 7, "started_at": "2026-11-14T00:00:10Z", "completed_at": "2026-11-14T00:00:10Z" } ]}),
        );
        let nums: Vec<u64> = steps.iter().map(|s| s["number"].as_u64().unwrap()).collect();
        assert_eq!(nums, [1, 2, 3]);
        assert_eq!(
            (steps[1]["status"].as_str(), steps[1]["conclusion"].as_str()),
            (Some("completed"), Some("failure"))
        );
        assert_eq!(steps[1]["completed_at"], "2026-11-14T00:00:09Z");
        assert_eq!(steps[2]["conclusion"], "skipped");

        let j = job(&c, &run, &running, "2026-11-14T00:00:00Z", &steps[..1], Some((5, "r-5")));
        for f in JOB_FIELDS {
            assert!(j.get(*f).is_some(), "job lacks {f}");
        }
        assert_eq!((j["status"].as_str(), j["completed_at"].as_str()), (Some("in_progress"), None));
        assert_eq!((j["run_id"].as_i64(), j["run_attempt"].as_u64()), (Some(9), Some(2)));
        assert_eq!(j["started_at"], "2026-11-14T00:00:04Z");
        assert_eq!(j["labels"], json!(["linux", "self-hosted"]));
        let k = job(&c, &run, &done, "2026-11-14T00:00:00Z", &[], None);
        assert_eq!((k["status"].as_str(), k["conclusion"].as_str()), (Some("completed"), Some("failure")));
        assert_eq!(k["completed_at"], graftyard_wire::time::iso(Timestamp(1_763_100_000_000)));
        assert!(k["runner_id"].is_null());
        // A job whose needs are unmet is GHES's `waiting`; one matched but not acquired is still `queued`.
        assert_eq!(job_status(&JobState::Pending { needs: Default::default() }).0, "waiting");
        assert_eq!(
            job_status(&JobState::Assigned { runner: RunnerId(DbId(5)), lease_until: Timestamp(1) }).0,
            "queued"
        );

        // `agent-ci`: a run held for approval reads as GitHub's run awaiting approval, with no jobs listed; once its
        // jobs are released it reads as any other.
        let held = [row(12, JobState::Held { needs: Default::default(), since: Timestamp(1) })];
        let h = workflow_run(&c, &run, &held, "2026-11-14T00:00:00Z", "2026-11-14T00:00:00Z");
        assert_eq!((h["status"].as_str(), h["conclusion"].as_str()), (Some("completed"), Some("action_required")));
        assert_eq!(visible_jobs(&held).count(), 0);
        let both = [done.clone(), held[0].clone()];
        assert_eq!(visible_jobs(&both).map(|j| j.spec.id).collect::<Vec<_>>(), [done.spec.id]);

        // `secrets-wait` (0x15.14): a job waiting for `AgentGate` is listed and reads GHES's `waiting`, as a job waiting
        // on a protection rule does, and its run reads `queued` beside a job not started yet, `in_progress` beside a
        // running one. The job's body is written for `crates/api/gh/validate.py` when `GRAFTYARD_REPLIES` is set.
        let wait = row(13, JobState::SecretsWait { needs: Default::default(), since: Timestamp(1), head: [7; 20] });
        let w = job(&c, &run, &wait, "2026-11-14T00:00:00Z", &[], None);
        assert_eq!((w["status"].as_str(), w["conclusion"].as_str()), (Some("waiting"), None));
        assert!(w["started_at"].is_string() && w["completed_at"].is_null() && w["runner_id"].is_null());
        assert_eq!(visible_jobs(std::slice::from_ref(&wait)).count(), 1);
        let r = workflow_run(&c, &run, std::slice::from_ref(&wait), "2026-11-14T00:00:00Z", "2026-11-14T00:00:00Z");
        assert_eq!((r["status"].as_str(), r["conclusion"].as_str()), (Some("queued"), None));
        let r2 =
            workflow_run(&c, &run, &[running.clone(), wait.clone()], "2026-11-14T00:00:00Z", "2026-11-14T00:00:00Z");
        assert_eq!(r2["status"], "in_progress");
        if let Ok(dir) = std::env::var("GRAFTYARD_REPLIES") {
            let v = json!({ "method": "GET", "path": "/repos/{owner}/{repo}/actions/jobs/{job_id}", "status": 200, "body": w });
            std::fs::write(format!("{dir}/secrets-wait-job.rest.json"), serde_json::to_vec_pretty(&v).unwrap())
                .unwrap();
        }
    }
}
