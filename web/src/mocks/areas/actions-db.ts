import type { Schemas } from "@/api/client";
import type { RelayLine, RelayMessage } from "@/areas/actions/ui-api";
import { failing } from "../faults";
import { branchSha, edge, raft, sha, users, web, type Repository, type SimpleUser } from "../world";
import { checkRun, db, type CheckRun } from "./core-db";
import { workflowSpecs, type Ctx, type JobSpec, type WorkflowSpec } from "./actions-workflows";

// Workflow runs, their jobs and logs, artifacts, deployment reviews, and the run engine.
// A job is a small state machine (queued, waiting, in_progress, completed) driven by one
// generator; seeded history runs it on a virtual clock, live runs on timers, so a
// dispatched run starts, streams its log, and finishes the same way the seeds did.

type WorkflowRun = Schemas["workflow-run"];
type Job = Schemas["job"];
type Artifact = Schemas["artifact"];
type Annotation = Schemas["check-annotation"];
type Workflow = Schemas["workflow"];

export type LogLine = RelayLine;

type PlannedStep = { name: string; lines: string[]; skip: boolean; fails: boolean };

export type JobRec = {
  job: Job;
  spec: JobSpec;
  run: RunRec;
  plan: PlannedStep[];
  log: LogLine[];
  annotations: Annotation[];
  check: CheckRun;
  listeners: Set<(m: RelayMessage) => void>;
  // Set by cancel; the driver stops at its next tick.
  cancelled: boolean;
  // A live job that has a runner streaming to its relay.
  streaming: boolean;
  // Handed to the broker: it starts when a runner with its labels is free.
  dispatched: boolean;
};

export type PendingEnv = {
  env: Environment;
  job: JobRec;
  since: string;
};

export type RunRec = {
  full: string;
  repo: Repository;
  spec: WorkflowSpec;
  workflow: Workflow;
  run: WorkflowRun;
  ctx: Ctx;
  // Jobs per attempt; the last is the current attempt.
  attempts: JobRec[][];
  snapshots: WorkflowRun[];
  artifacts: Artifact[];
  pending: PendingEnv[];
  approvals: Schemas["environment-approvals"][];
};

// ---- Environments (owned by the releases area; these ids are what pending deployments name) ----

type Environment = { id: number; name: string; reviewers: SimpleUser[]; wait_timer: number };
const ENVIRONMENTS: Environment[] = [
  { id: 161088068, name: "staging", reviewers: [], wait_timer: 0 },
  { id: 161088069, name: "production", reviewers: [users.mira, users.devi], wait_timer: 0 },
];
export const environments = (full: string) => (full === "graftyard/edge" ? ENVIRONMENTS : []);

// ---- Ids ----

let runSeq = 11_820_401;
let jobSeq = 31_002_100;
let artifactSeq = 2_480_100;

// ---- Store ----

const runs = new Map<string, RunRec[]>();
const workflows = new Map<string, { workflow: Workflow; spec: WorkflowSpec }[]>();
const jobsById = new Map<number, JobRec>();
const checkAnnotations = new Map<number, Annotation[]>();
const repoOf: Record<string, Repository> = {
  "graftyard/edge": edge,
  "graftyard/raft": raft,
  "graftyard/web": web,
};

const iso = (ms: number) => new Date(ms).toISOString();
// A stable pseudo-random number in [0, 1) for seeded durations.
const rand = (seed: string) => parseInt(sha(seed).slice(0, 8), 16) / 0x1_0000_0000;

function workflowRecord(repo: Repository, spec: WorkflowSpec, n: number): Workflow {
  const id = 7_100_000 + parseInt(sha(`${repo.full_name}:${spec.file}`).slice(0, 5), 16) + n;
  return {
    id,
    node_id: `W_${sha(`${repo.full_name}:${spec.file}`).slice(0, 12)}`,
    name: spec.name,
    path: `.github/workflows/${spec.file}`,
    state: spec.state ?? "active",
    created_at: spec.created,
    updated_at: spec.created,
    url: `${repo.url}/actions/workflows/${id}`,
    html_url: `${repo.html_url}/blob/main/.github/workflows/${spec.file}`,
    badge_url: `${repo.html_url}/workflows/${encodeURIComponent(spec.name)}/badge.svg`,
  };
}

for (const [full, specs] of Object.entries(workflowSpecs)) {
  workflows.set(
    full,
    specs.map((spec, i) => ({ workflow: workflowRecord(repoOf[full], spec, i), spec })),
  );
  runs.set(full, []);
}

function minimal(repo: Repository): Schemas["minimal-repository"] {
  const { license, ...rest } = repo;
  return {
    ...rest,
    license: license && {
      key: license.key,
      name: license.name,
      url: license.url,
      node_id: license.node_id,
      spdx_id: license.spdx_id ?? undefined,
    },
  };
}

// ---- Planning: what each step prints, and where the job fails ----

function planJob(
  spec: JobSpec,
  ctx: Ctx,
): { plan: PlannedStep[]; annotations: Annotation[]; failed: boolean } {
  const failure = spec.fails?.(ctx) ?? null;
  const last = spec.steps.length - 1;
  const plan = spec.steps.map((s, i) => {
    const fails = failure?.step === i;
    const skip = Boolean(failure && i > failure.step && i !== last);
    const lines = fails
      ? [
          ...s
            .lines(ctx)
            .filter((l) => l.startsWith("##[group]") || l === "##[endgroup]")
            .slice(0, 2),
          ...failure!.lines,
        ]
      : skip
        ? []
        : s.lines(ctx);
    return { name: s.name, lines, skip, fails };
  });
  const repo = repoOf[ctx.full];
  const annotations = (failure?.annotations ?? []).map((a) => ({
    ...a,
    blob_href: `${repo.html_url}/blob/${ctx.sha}/${a.path}`,
  }));
  return { plan, annotations, failed: Boolean(failure) };
}

// ---- Check runs: a job is a check run on the head sha, so a PR's Checks region shows it ----

function bindCheckRun(rec: RunRec, spec: JobSpec, url: string): CheckRun {
  const sha = rec.run.head_sha;
  const tip = rec.run.head_branch && branchSha(rec.full, rec.run.head_branch) === sha;
  // A re-run reuses the check run of the same name, so a PR shows one row per job.
  const existing = db.checkRuns(sha).find((c) => c.name === spec.name);
  const check =
    existing ??
    checkRun(rec.repo, sha, spec.name, "queued", null, rec.run.created_at, null, "Queued");
  check.details_url = check.html_url = url;
  if (!existing && tip) db.addCheckRun(sha, check);
  return check;
}

function syncCheck(j: JobRec) {
  const c = j.check;
  c.status =
    j.job.status === "completed"
      ? "completed"
      : j.job.status === "in_progress"
        ? "in_progress"
        : j.job.status === "waiting"
          ? "waiting"
          : "queued";
  c.conclusion = j.job.conclusion;
  c.started_at = j.job.started_at;
  c.completed_at = j.job.completed_at;
  const done = j.plan.filter((s) => !s.skip).length;
  const summary =
    j.job.status === "completed"
      ? j.job.conclusion === "success"
        ? `${done} steps passed`
        : j.job.conclusion === "failure"
          ? (j.annotations[0]?.message ?? "Process completed with exit code 1.")
          : (j.job.conclusion ?? "")
      : j.job.status === "in_progress"
        ? "Running"
        : j.job.status === "waiting"
          ? "Waiting for review"
          : "Waiting for a runner";
  c.output.title = c.output.summary = summary;
  c.output.annotations_count = j.annotations.length;
  c.output.annotations_url = `${j.run.repo.url}/check-runs/${c.id}/annotations`;
  checkAnnotations.set(c.id, j.annotations);
}

export const annotationsFor = (checkRunId: number) => checkAnnotations.get(checkRunId);
export const isActionsCheckRun = (checkRunId: number) => checkAnnotations.has(checkRunId);

// ---- Runners: a job needs an online runner with every label it asks for ----

export type RunnerRec = Schemas["runner"] & { scope: "repo" | "org"; repo?: string };

let runnerSeq = 40;
const label = (name: string, type: "read-only" | "custom" = "custom") => ({
  id: name.length * 7 + (type === "custom" ? 100 : 0),
  name,
  type,
});

export function runner(
  name: string,
  status: "online" | "offline",
  labels: string[],
  scope: RunnerRec["scope"],
  repo?: string,
  group = 1,
): RunnerRec {
  return {
    id: runnerSeq++,
    runner_group_id: group,
    name,
    os: labels.includes("macOS") ? "macOS" : "Linux",
    status,
    busy: false,
    ephemeral: false,
    version: "2.329.0",
    labels: [
      label("self-hosted", "read-only"),
      label(labels.includes("macOS") ? "macOS" : "Linux", "read-only"),
      label("X64", "read-only"),
      ...labels.filter((l) => !["macOS"].includes(l)).map((l) => label(l)),
    ],
    scope,
    repo,
  };
}

export const runners: RunnerRec[] = [
  runner("edge-runner-01", "online", ["linux", "x64"], "repo", "graftyard/edge"),
  runner("edge-runner-02", "online", ["linux", "x64"], "repo", "graftyard/edge"),
  runner("org-runner-01", "online", ["linux", "x64"], "org"),
  runner("org-runner-02", "online", ["linux", "x64"], "org"),
  runner("gpu-runner-01", "offline", ["linux", "x64", "gpu"], "org", undefined, 2),
  runner("mac-builder", "offline", ["macOS", "arm64"], "org"),
];

function pickRunner(full: string, wanted: string[]) {
  const want = wanted.map((l) => l.toLowerCase());
  return runners.find(
    (r) =>
      r.status === "online" &&
      !r.busy &&
      (r.scope === "org" || r.repo === full) &&
      want.every((l) => r.labels.some((x) => x.name.toLowerCase() === l)),
  );
}

// ---- The job state machine ----

function emit(j: JobRec, m: RelayMessage) {
  for (const l of j.listeners) l(m);
}

function touch(rec: RunRec, at: string) {
  rec.run.updated_at = at;
}

type Pace = { line: () => number; step: () => number };

// Steps one job from start to its conclusion. `now` is the clock: virtual for seeds,
// Date.now for live jobs. Yields the delay before the next tick.
function* drive(j: JobRec, now: () => number, pace: Pace): Generator<number, void> {
  const job = j.job;
  job.status = "in_progress";
  job.started_at = iso(now());
  job.steps = j.plan.map((s, i) => ({
    name: s.name,
    number: i + 1,
    status: "queued",
    conclusion: null,
    started_at: null,
    completed_at: null,
  }));
  syncCheck(j);
  touch(j.run, job.started_at);
  refreshRun(j.run);
  for (const [i, step] of j.plan.entries()) {
    const s = job.steps[i];
    if (j.cancelled) break;
    if (step.skip) {
      s.status = "completed";
      s.conclusion = "skipped";
      continue;
    }
    s.status = "in_progress";
    s.started_at = iso(now());
    emit(j, { type: "step", number: s.number, status: "in_progress", conclusion: null });
    for (const text of step.lines) {
      yield pace.line();
      if (j.cancelled) break;
      const line: LogLine = { seq: j.log.length, step: s.number, at: iso(now()), text };
      j.log.push(line);
      // A killed relay drops its viewers; the job keeps running and its blob keeps growing.
      if (j.streaming && failing("actions-log-relay")) {
        emit(j, { type: "no-stream" });
        j.listeners.clear();
      }
      emit(j, { type: "line", line });
    }
    if (j.cancelled) break;
    yield pace.step();
    s.status = "completed";
    s.conclusion = step.fails ? "failure" : "success";
    s.completed_at = iso(now());
    emit(j, { type: "step", number: s.number, status: "completed", conclusion: s.conclusion });
  }
  const at = iso(now());
  if (j.cancelled) {
    for (const s of job.steps) {
      if (s.status === "completed") continue;
      s.status = "completed";
      s.conclusion = s.started_at ? "cancelled" : "skipped";
      s.completed_at = s.started_at ? at : null;
    }
    j.log.push({
      seq: j.log.length,
      step: job.steps.find((s) => s.conclusion === "cancelled")?.number ?? 1,
      at,
      text: "##[error]The operation was canceled.",
    });
  }
  job.status = "completed";
  job.conclusion = j.cancelled ? "cancelled" : j.plan.some((s) => s.fails) ? "failure" : "success";
  job.completed_at = at;
  releaseRunner(j);
  syncCheck(j);
  emit(j, { type: "end", conclusion: job.conclusion });
  j.listeners.clear();
  j.streaming = false;
  if (job.conclusion === "success" && j.spec.artifact) addArtifact(j.run, j.spec.artifact, at);
  touch(j.run, at);
}

function releaseRunner(j: JobRec) {
  const r = runners.find((x) => x.id === j.job.runner_id);
  if (r) r.busy = false;
}

function assignRunner(j: JobRec): boolean {
  const r = pickRunner(j.run.full, j.spec.runsOn);
  if (!r) return false;
  r.busy = true;
  j.job.runner_id = r.id;
  j.job.runner_name = r.name;
  j.job.runner_group_id = r.runner_group_id ?? 1;
  j.job.runner_group_name = r.runner_group_id === 2 ? "GPU" : "Default";
  return true;
}

const LIVE: Pace = { line: () => 90 + Math.random() * 60, step: () => 350 };
const historyPace = (seed: string): Pace => {
  let n = 0;
  return {
    line: () => 40 + rand(`${seed}:l:${n++}`) * 400,
    step: () => 800 + rand(`${seed}:s:${n++}`) * 6000,
  };
};

// Runs a job on timers, so its lines reach the relay as they happen.
function startLive(j: JobRec) {
  j.dispatched = true;
  if (!assignRunner(j)) {
    // No idle runner with these labels: stay queued and ask again, as the broker would.
    setTimeout(() => {
      if (!j.cancelled && j.job.status === "queued" && actions.job(j.job.id)) startLive(j);
    }, 3000);
    return;
  }
  j.streaming = true;
  const steps = drive(j, Date.now, LIVE);
  const tick = () => {
    const next = steps.next();
    if (next.done) return advance(j.run);
    setTimeout(tick, next.value);
  };
  tick();
}

function runVirtual(j: JobRec, start: number): number {
  let t = start;
  assignRunner(j);
  const steps = drive(j, () => t, historyPace(`${j.job.id}`));
  for (let next = steps.next(); !next.done; next = steps.next()) t += next.value;
  return t;
}

// ---- Run status: derived from its jobs, never stored apart from them ----

function refreshRun(rec: RunRec) {
  const jobs = current(rec).map((j) => j.job);
  const run = rec.run;
  if (jobs.every((j) => j.status === "completed")) {
    run.status = "completed";
    run.conclusion = jobs.some((j) => j.conclusion === "cancelled")
      ? "cancelled"
      : jobs.some((j) => j.conclusion === "failure")
        ? "failure"
        : "success";
    return;
  }
  run.conclusion = null;
  // Between two jobs (one finished, the next not yet on a runner) the run is still in
  // progress; before any job of this attempt has run, it is queued.
  const begun = jobs.some(
    (j) => j.completed_at && run.run_started_at && j.completed_at >= run.run_started_at,
  );
  run.status = jobs.some((j) => j.status === "in_progress")
    ? "in_progress"
    : jobs.some((j) => j.status === "waiting")
      ? "waiting"
      : begun
        ? "in_progress"
        : "queued";
}

const current = (rec: RunRec) => rec.attempts[rec.attempts.length - 1];

// Starts every queued job whose needs have finished, or skips it when one failed.
// `mode` decides how a ready job runs: live on timers, or on the virtual clock.
function advance(
  rec: RunRec,
  mode:
    | "live"
    | { at: (key: string) => number; virtual: (j: JobRec) => "run" | "live" | "wait" } = "live",
) {
  let progressed = true;
  while (progressed) {
    progressed = false;
    for (const j of current(rec)) {
      if (j.job.status !== "queued" || j.cancelled || j.dispatched) continue;
      const needs = (j.spec.needs ?? []).map((k) => current(rec).find((x) => x.spec.key === k)!);
      if (needs.some((n) => n.job.status !== "completed")) continue;
      progressed = true;
      if (j.spec.when && !j.spec.when(rec.ctx)) {
        skip(j, "skipped");
        continue;
      }
      if (needs.some((n) => n.job.conclusion !== "success")) {
        skip(j, needs.some((n) => n.job.conclusion === "cancelled") ? "cancelled" : "skipped");
        continue;
      }
      if (mode === "live") {
        if (j.spec.environment && needsReview(j)) waitForReview(j, iso(Date.now()));
        else startLive(j);
        continue;
      }
      const start = Math.max(
        ...needs.map((n) => Date.parse(n.job.completed_at!)),
        mode.at(j.spec.key),
      );
      const how = mode.virtual(j);
      if (how === "wait") waitForReview(j, iso(start + 2000));
      else if (how === "live") startLive(j);
      else {
        if (j.spec.environment && needsReview(j))
          recordApproval(rec, j, users.devi, "approved", "Ship it.", iso(start + 60_000));
        runVirtual(j, start + (j.spec.environment && needsReview(j) ? 90_000 : 4000));
      }
    }
  }
  refreshRun(rec);
  if (rec.run.status === "completed" && mode === "live") touch(rec, iso(Date.now()));
}

function skip(j: JobRec, conclusion: "skipped" | "cancelled") {
  const at = j.run.run.updated_at;
  j.job.status = "completed";
  j.job.conclusion = conclusion;
  j.job.started_at = at;
  j.job.completed_at = at;
  j.job.steps = [];
  syncCheck(j);
}

const needsReview = (j: JobRec) =>
  (environments(j.run.full).find((e) => e.name === j.spec.environment)?.reviewers.length ?? 0) > 0;

function waitForReview(j: JobRec, at: string) {
  const env = environments(j.run.full).find((e) => e.name === j.spec.environment)!;
  j.job.status = "waiting";
  syncCheck(j);
  j.run.pending.push({ env, job: j, since: at });
  touch(j.run, at);
  refreshRun(j.run);
}

function recordApproval(
  rec: RunRec,
  j: JobRec,
  user: SimpleUser,
  state: "approved" | "rejected",
  comment: string,
  at: string,
) {
  const env = environments(rec.full).find((e) => e.name === j.spec.environment)!;
  rec.approvals.push({
    environments: [envRef(rec.repo, env, at)],
    state,
    user,
    comment,
  });
}

const envRef = (repo: Repository, env: Environment, at: string) => ({
  id: env.id,
  node_id: `EN_${sha(`env:${env.id}`).slice(0, 12)}`,
  name: env.name,
  url: `${repo.url}/environments/${env.name}`,
  html_url: `${repo.html_url}/deployments/activity_log?environments_filter=${env.name}`,
  created_at: "2025-04-11T12:00:00Z",
  updated_at: at,
});

function addArtifact(rec: RunRec, a: { name: string; size: number }, at: string) {
  const id = artifactSeq++;
  rec.artifacts = rec.artifacts.filter((x) => x.name !== a.name);
  rec.artifacts.push({
    id,
    node_id: `MDg6QXJ0aWZhY3Q${id}`,
    name: a.name,
    size_in_bytes: a.size + (id % 997),
    url: `${rec.repo.url}/actions/artifacts/${id}`,
    archive_download_url: `${rec.repo.url}/actions/artifacts/${id}/zip`,
    expired: false,
    created_at: at,
    updated_at: at,
    expires_at: iso(Date.parse(at) + 90 * 86_400_000),
    workflow_run: {
      id: rec.run.id,
      repository_id: rec.repo.id,
      head_repository_id: rec.repo.id,
      head_branch: rec.run.head_branch ?? undefined,
      head_sha: rec.run.head_sha,
    },
  });
}

// ---- Creating runs and attempts ----

type RunSeed = {
  full: string;
  file: string;
  event: string;
  branch: string;
  sha?: string;
  actor: SimpleUser;
  created: string;
  title: string;
  pull?: number;
  inputs?: Record<string, string | boolean>;
};

function newJobs(rec: RunRec, keys?: Set<string>, keep?: JobRec[]): JobRec[] {
  return rec.spec.jobs.map((spec) => {
    const kept = keep?.find((k) => k.spec.key === spec.key);
    if (kept && keys && !keys.has(spec.key)) {
      // A job a partial re-run keeps: GHES copies it into the new attempt as it was.
      const copy: JobRec = {
        ...kept,
        job: { ...kept.job, run_attempt: rec.run.run_attempt },
        listeners: new Set(),
      };
      return copy;
    }
    const id = jobSeq++;
    const { plan, annotations } = planJob(spec, rec.ctx);
    const url = `${rec.repo.html_url}/actions/runs/${rec.run.id}/job/${id}`;
    const j: JobRec = {
      spec,
      run: rec,
      plan,
      log: [],
      annotations,
      check: undefined as unknown as CheckRun,
      listeners: new Set(),
      cancelled: false,
      streaming: false,
      dispatched: false,
      job: {
        id,
        run_id: rec.run.id,
        run_url: rec.run.url,
        run_attempt: rec.run.run_attempt,
        node_id: `CR_${sha(`job:${id}`).slice(0, 12)}`,
        head_sha: rec.run.head_sha,
        url: `${rec.repo.url}/actions/jobs/${id}`,
        html_url: url,
        status: "queued",
        conclusion: null,
        created_at: rec.run.updated_at,
        started_at: rec.run.updated_at,
        completed_at: null,
        name: spec.name,
        steps: [],
        check_run_url: "",
        labels: spec.runsOn,
        runner_id: null,
        runner_name: null,
        runner_group_id: null,
        runner_group_name: null,
        workflow_name: rec.spec.name,
        head_branch: rec.run.head_branch,
      },
    };
    j.check = bindCheckRun(rec, spec, url);
    j.job.check_run_url = `${rec.repo.url}/check-runs/${j.check.id}`;
    syncCheck(j);
    jobsById.set(id, j);
    return j;
  });
}

function createRun(s: RunSeed): RunRec {
  const repo = repoOf[s.full];
  const { workflow, spec } = workflows.get(s.full)!.find((w) => w.spec.file === s.file)!;
  const list = runs.get(s.full)!;
  const id = runSeq++;
  const headSha = s.sha ?? branchSha(s.full, s.branch);
  const base = `${repo.url}/actions/runs/${id}`;
  const pr = s.pull
    ? db.state("graftyard", repo.name)?.pulls.find((p) => p.number === s.pull)
    : undefined;
  const run: WorkflowRun = {
    id,
    name: spec.name,
    node_id: `WFR_${sha(`run:${id}`).slice(0, 12)}`,
    check_suite_id: id + 900_000,
    check_suite_node_id: `CS_${sha(`cs:${id}`).slice(0, 12)}`,
    head_branch: s.branch,
    head_sha: headSha,
    path: workflow.path,
    run_number: list.filter((r) => r.workflow.id === workflow.id).length + 1,
    run_attempt: 1,
    referenced_workflows: [],
    event: s.event,
    status: "queued",
    conclusion: null,
    workflow_id: workflow.id,
    url: base,
    html_url: `${repo.html_url}/actions/runs/${id}`,
    pull_requests: pr
      ? [
          {
            id: pr.id,
            number: pr.number,
            url: pr.url,
            head: {
              ref: pr.head.ref,
              sha: pr.head.sha,
              repo: { id: repo.id, url: repo.url, name: repo.name },
            },
            base: {
              ref: pr.base.ref,
              sha: pr.base.sha,
              repo: { id: repo.id, url: repo.url, name: repo.name },
            },
          },
        ]
      : [],
    created_at: s.created,
    updated_at: s.created,
    actor: s.actor,
    triggering_actor: s.actor,
    run_started_at: s.created,
    jobs_url: `${base}/jobs`,
    logs_url: `${base}/logs`,
    check_suite_url: `${repo.url}/check-suites/${id + 900_000}`,
    artifacts_url: `${base}/artifacts`,
    cancel_url: `${base}/cancel`,
    rerun_url: `${base}/rerun`,
    previous_attempt_url: null,
    workflow_url: workflow.url,
    head_commit: {
      id: headSha,
      tree_id: sha(`tree:${headSha}`),
      message: s.title,
      timestamp: s.created,
      author: { name: s.actor.name ?? s.actor.login, email: `${s.actor.login}@graftyard.test` },
      committer: { name: s.actor.name ?? s.actor.login, email: `${s.actor.login}@graftyard.test` },
    },
    repository: minimal(repo),
    head_repository: minimal(repo),
    head_repository_id: repo.id,
    display_title: s.title,
  };
  const rec: RunRec = {
    full: s.full,
    repo,
    spec,
    workflow,
    run,
    ctx: {
      full: s.full,
      branch: s.branch,
      sha: headSha,
      attempt: 1,
      event: s.event,
      inputs: s.inputs ?? {},
    },
    attempts: [],
    snapshots: [],
    artifacts: [],
    pending: [],
    approvals: [],
  };
  rec.attempts.push(newJobs(rec));
  list.unshift(rec);
  return rec;
}

// Plays a run's jobs on the virtual clock from its creation. `virtual` may stop a job
// short: "live" starts it on timers now, and "wait" parks it on an environment review.
function simulate(rec: RunRec, virtual: (j: JobRec) => "run" | "live" | "wait" = () => "run") {
  const t0 = Date.parse(rec.run.created_at) + 3000 + rand(`${rec.run.id}:q`) * 9000;
  rec.run.run_started_at = iso(t0);
  advance(rec, { at: () => t0, virtual });
  const last = Math.max(
    ...current(rec).map((j) => Date.parse(j.job.completed_at ?? j.job.started_at)),
  );
  touch(rec, iso(last));
  refreshRun(rec);
}

// ---- What the handlers call ----

export const actions = {
  workflows: (full: string) => workflows.get(full) ?? [],
  workflow: (full: string, idOrFile: string) =>
    workflows
      .get(full)
      ?.find((w) => String(w.workflow.id) === idOrFile || w.spec.file === idOrFile),
  runs: (full: string) => runs.get(full) ?? [],
  run: (full: string, id: number) => runs.get(full)?.find((r) => r.run.id === id),
  job: (id: number) => jobsById.get(id),
  jobs: (rec: RunRec, attempt?: number) => rec.attempts[(attempt ?? rec.attempts.length) - 1] ?? [],
  attemptRun: (rec: RunRec, attempt: number) =>
    attempt === rec.run.run_attempt ? rec.run : rec.snapshots[attempt - 1],

  dispatch(
    full: string,
    file: string,
    branch: string,
    inputs: Record<string, string | boolean>,
  ): RunRec {
    const rec = createRun({
      full,
      file,
      event: "workflow_dispatch",
      branch,
      actor: users.mira,
      created: iso(Date.now()),
      title: workflows.get(full)!.find((w) => w.spec.file === file)!.spec.name,
      inputs,
    });
    // A dispatched run waits in the queue for a moment, then a runner takes it.
    setTimeout(() => {
      rec.run.run_started_at = iso(Date.now());
      advance(rec);
    }, 2500);
    return rec;
  },

  // A new attempt. `keys` names the jobs to run again (with their dependents); the rest
  // are copied as they finished. No keys: every job runs again.
  rerun(rec: RunRec, keys?: Set<string>) {
    const prev = current(rec);
    rec.snapshots.push(structuredClone(rec.run));
    rec.run.run_attempt = (rec.run.run_attempt ?? 1) + 1;
    rec.run.previous_attempt_url = `${rec.run.url}/attempts/${rec.run.run_attempt - 1}`;
    rec.ctx = { ...rec.ctx, attempt: rec.run.run_attempt };
    const now = iso(Date.now());
    rec.run.run_started_at = now;
    touch(rec, now);
    let wanted = keys;
    if (wanted) {
      // Dependents of a re-run job run again too.
      let grew = true;
      while (grew) {
        grew = false;
        for (const s of rec.spec.jobs)
          if (!wanted.has(s.key) && (s.needs ?? []).some((n) => wanted!.has(n))) {
            wanted.add(s.key);
            grew = true;
          }
      }
    }
    rec.pending = [];
    rec.attempts.push(newJobs(rec, wanted, prev));
    refreshRun(rec);
    setTimeout(() => advance(rec), 1500);
  },

  failedKeys: (rec: RunRec) =>
    new Set(
      current(rec)
        .filter((j) => j.job.conclusion === "failure" || j.job.conclusion === "cancelled")
        .map((j) => j.spec.key),
    ),

  cancel(rec: RunRec) {
    for (const j of current(rec)) if (j.job.status !== "completed") j.cancelled = true;
    rec.pending = [];
    // Jobs that never started end at once; running ones stop at their next tick.
    setTimeout(() => {
      for (const j of current(rec)) {
        if (j.job.status === "queued" || j.job.status === "waiting") skip(j, "cancelled");
      }
      refreshRun(rec);
      touch(rec, iso(Date.now()));
    }, 600);
  },

  deleteRun(rec: RunRec) {
    runs.set(
      rec.full,
      runs.get(rec.full)!.filter((r) => r !== rec),
    );
    for (const j of rec.attempts.flat()) jobsById.delete(j.job.id);
  },

  review(rec: RunRec, envIds: number[], state: "approved" | "rejected", comment: string) {
    const now = iso(Date.now());
    const reviewed = rec.pending.filter((p) => envIds.includes(p.env.id));
    rec.pending = rec.pending.filter((p) => !envIds.includes(p.env.id));
    for (const p of reviewed) {
      recordApproval(rec, p.job, users.mira, state, comment, now);
      if (state === "approved") {
        p.job.job.status = "queued";
        startLive(p.job);
      } else {
        p.job.job.status = "completed";
        p.job.job.conclusion = "failure";
        p.job.job.completed_at = now;
        p.job.log.push({
          seq: p.job.log.length,
          step: 1,
          at: now,
          text: `##[error]Deployment to ${p.env.name} was rejected by mira: ${comment || "no comment"}`,
        });
        syncCheck(p.job);
      }
    }
    refreshRun(rec);
    touch(rec, now);
    if (state === "rejected") advance(rec);
    return reviewed;
  },

  deployment(rec: RunRec, p: PendingEnv): Schemas["deployment"] {
    const id = 90_000 + (p.env.id % 1000) + (rec.run.id % 1000);
    return {
      url: `${rec.repo.url}/deployments/${id}`,
      id,
      node_id: `DE_${sha(`dep:${id}`).slice(0, 12)}`,
      sha: rec.run.head_sha,
      ref: rec.run.head_branch ?? "main",
      task: "deploy",
      payload: {},
      original_environment: p.env.name,
      environment: p.env.name,
      description: `Deploy ${rec.run.head_sha.slice(0, 7)} from run ${rec.run.run_number}`,
      creator: users.mira,
      created_at: p.since,
      updated_at: iso(Date.now()),
      statuses_url: `${rec.repo.url}/deployments/${id}/statuses`,
      repository_url: rec.repo.url,
      production_environment: p.env.name === "production",
    };
  },

  pendingFor: (rec: RunRec): Schemas["pending-deployment"][] =>
    rec.pending.map((p) => ({
      environment: envRef(rec.repo, p.env, p.since),
      wait_timer: p.env.wait_timer,
      wait_timer_started_at: p.since,
      current_user_can_approve: p.env.reviewers.some((u) => u.login === "mira"),
      reviewers: p.env.reviewers.map((u) => ({ type: "User" as const, reviewer: u })),
    })),

  artifacts: (full: string) => (runs.get(full) ?? []).flatMap((r) => r.artifacts),
  deleteArtifact(full: string, id: number) {
    for (const r of runs.get(full) ?? []) r.artifacts = r.artifacts.filter((a) => a.id !== id);
  },
  artifact: (id: number) =>
    [...runs.values()]
      .flat()
      .flatMap((r) => r.artifacts)
      .find((a) => a.id === id),

  // The blob a job's logs URL redirects to: one line per log line, timestamp first.
  logText: (j: JobRec) => j.log.map((l) => `${l.at} ${l.text}`).join("\n"),

  setWorkflowState(full: string, idOrFile: string, state: Workflow["state"]) {
    const w = actions.workflow(full, idOrFile);
    if (!w) return false;
    w.workflow.state = state;
    w.workflow.updated_at = iso(Date.now());
    return true;
  },
};

// ---- Seed history ----

const DAY = 86_400_000;
const NOW = Date.now();
const ago = (ms: number) => iso(NOW - ms);

const MAIN_COMMITS: [string, SimpleUser][] = [
  ["Send rate limit headers on every error response (#5)", users.devi],
  ["Merge the learner read-index experiment notes", users.lin],
  ["wire: answer 304 when If-None-Match matches", users.devi],
  ["raft: renew leases a tick earlier", users.lin],
  ["edge: parse an empty per_page as the default", users.tomas],
  ["api: list pull requests in the issues endpoint", users.tomas],
  ["docs: the retry budget for idempotent reads", users.mira],
  ["deps: bump workers-rs to 0.6.4", users.mira],
  ["link: omit rel=last on a single page", users.tomas],
  ["Label colours keep their leading hash in the web app", users.lin],
  ["Timeline events for label changes", users.tomas],
  ["edge: cap per_page at 100", users.devi],
];

function seed() {
  const full = "graftyard/edge";
  // Oldest first, so run numbers count up.
  const seeds: (RunSeed & { then?: (rec: RunRec) => void })[] = [];
  MAIN_COMMITS.forEach(([title, actor], i) => {
    const created = ago((12 - i) * 0.8 * DAY + 3_600_000 * (i % 5));
    const commitSha = i === MAIN_COMMITS.length - 1 ? undefined : sha(`${full}:main:${i}`);
    seeds.push({
      full,
      file: "ci.yml",
      event: "push",
      branch: "main",
      sha: commitSha,
      actor,
      created,
      title,
    });
    if (i % 3 === 1 || i === MAIN_COMMITS.length - 1)
      seeds.push({
        full,
        file: "deploy.yml",
        event: "push",
        branch: "main",
        sha: commitSha,
        actor,
        created: iso(Date.parse(created) + 60_000),
        title,
      });
  });
  for (let d = 9; d >= 1; d--)
    seeds.push({
      full,
      file: "chaos.yml",
      event: "schedule",
      branch: "main",
      actor: users.mira,
      created: iso(Date.parse("2026-11-08T03:17:00Z") - d * DAY),
      title: "Chaos",
      sha: sha(`${full}:main:chaos:${d}`),
    });
  for (let d = 5; d >= 1; d--)
    seeds.push({
      full,
      file: "stale.yml",
      event: "schedule",
      branch: "main",
      actor: users.mira,
      created: iso(Date.parse("2026-10-05T06:00:00Z") - d * 7 * DAY),
      title: "Stale issues",
      sha: sha(`${full}:main:stale:${d}`),
    });
  seeds.push(
    {
      full,
      file: "chaos.yml",
      event: "workflow_dispatch",
      branch: "main",
      actor: users.devi,
      created: ago(2.2 * DAY),
      title: "Chaos",
      inputs: { duration: "1h", sites: "weur-1,apac-1" },
      sha: sha(`${full}:main:chaos:dispatch`),
    },
    {
      full,
      file: "ci.yml",
      event: "push",
      branch: "release/0.4",
      actor: users.mira,
      created: ago(1.6 * DAY),
      title: "release: 0.4.0-rc.1",
    },
    {
      full,
      file: "ci.yml",
      event: "pull_request",
      branch: "devi/ratelimit-on-errors",
      actor: users.devi,
      created: "2026-11-04T10:02:00Z",
      title: "Send rate limit headers on every error response",
      pull: 5,
    },
    {
      full,
      file: "ci.yml",
      event: "pull_request",
      branch: "tomas/label-objects",
      sha: sha(`${full}:tomas/label-objects:1`),
      actor: users.tomas,
      created: "2026-11-06T08:05:00Z",
      title: "Accept label objects on issue create",
      pull: 14,
    },
    {
      full,
      file: "ci.yml",
      event: "pull_request",
      branch: "tomas/label-objects",
      actor: users.tomas,
      created: "2026-11-07T08:40:00Z",
      title: "Accept label objects on issue create",
      pull: 14,
    },
    {
      full,
      file: "ci.yml",
      event: "pull_request",
      branch: "mira/idempotent-retry",
      actor: users.mira,
      created: ago(6 * 60_000),
      title: "Retry only idempotent reads at the edge client",
      pull: 1,
    },
    {
      full,
      file: "bench-gpu.yml",
      event: "pull_request",
      branch: "lin/learner-reads",
      actor: users.lin,
      created: ago(40_000),
      title: "Draft: learner read-index for hot issue lists",
      pull: 9,
    },
    {
      full,
      file: "ci.yml",
      event: "pull_request",
      branch: "lin/learner-reads",
      actor: users.lin,
      created: ago(40_000),
      title: "Draft: learner read-index for hot issue lists",
      pull: 9,
    },
  );
  seeds.sort((a, b) => a.created.localeCompare(b.created));

  const latestDeploy = seeds.filter((s) => s.file === "deploy.yml").at(-1);
  for (const s of seeds) {
    const rec = createRun(s);
    if (s.pull === 1) {
      // PR #1's byte-diff is still running when the page loads; it streams live.
      simulate(rec, (j) => (j.spec.key === "byte-diff" ? "live" : "run"));
    } else if (s.pull === 9) {
      // Queued: the CI run gets a runner a few seconds after load; the GPU bench never
      // does, because the only gpu runner is offline.
      if (s.file === "ci.yml")
        setTimeout(() => ((rec.run.run_started_at = iso(Date.now())), advance(rec)), 6000);
    } else if (s === latestDeploy) {
      simulate(rec, (j) => (j.spec.key === "production" ? "wait" : "run"));
    } else {
      simulate(rec);
    }
  }

  // raft and web: a few days of CI on their branches.
  for (const [repoFull, titles] of [
    [
      "graftyard/raft",
      [
        "raft: pre-vote with two restarts",
        "transfer: hand off leadership on drain",
        "joint consensus: first cut",
        "log: compact after snapshot",
      ],
    ],
    [
      "graftyard/web",
      ["web: dark diff colours", "web: actions run page", "web: mock panel lists gated regions"],
    ],
  ] as const) {
    titles.forEach((title, i) => {
      const branch = branchesOf(repoFull)[i % 2];
      const rec = createRun({
        full: repoFull,
        file: "ci.yml",
        event: i % 2 ? "pull_request" : "push",
        branch,
        sha: i >= titles.length - 2 ? undefined : sha(`${repoFull}:${i}`),
        actor: [users.lin, users.tomas, users.devi][i % 3],
        created: ago((titles.length - i) * 0.9 * DAY),
        title,
      });
      simulate(rec);
    });
  }
}

const branchesOf = (full: string) =>
  full === "graftyard/raft" ? ["main", "lin/joint-consensus"] : ["main", "lin/dark-diff"];

seed();
