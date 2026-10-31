// M0-WP14 (L0-04): the policy part of agent CI under `AgentPolicy.ci` (design/AGENT-LAYER.md, Shape, "Agent actors in
// the layer": `AgentPolicy`, `CiMode`; A3 step 9). No Actions engine exists yet, so this is only the decision a
// dispatcher makes for a workflow run that an agent push triggers. It covers the run's state, the token permissions,
// and, per job, whether it starts and which secrets it gets. It also covers the release of secrets when `AgentGate`
// later holds for that head. Pure functions over the fold's PR row and policy, with no clock and no I/O.
//
// The safety rule is structural. A job's secret map is filled only in `release`, and `release` checks
// `gateHolds(pr, job.head)` at that moment. `secretRefs` decides only whether a job waits or starts. A job that starts
// before the gate always gets an empty secret map, so if the scanner misses a reference, the job fails for lack of a
// secret and leaks nothing.

import type { AgentGate, AgentPolicy, Oid, Perm, PermSet, Pr, UserId } from "../agent/model.ts";

export interface Job { name: string; text: string /* the job's YAML, steps and env included */; permissions: PermSet | "write-all" | null }
export interface Workflow { name: string; jobs: Job[]; secrets: string[] /* the repository's secret names */ }

/** Who caused the run. `agentPush` is an `ActorKind::Agent` push or a push to a PR that carries `AgentMeta`. */
export interface Trigger { head: Oid; agentPush: boolean; runApprovedBy: UserId | null /* a writer's run approval, under Approve */ }

export type JobState = { t: "Queued" } | { t: "Waiting"; reason: string } | { t: "ActionRequired" };
export interface JobPlan { name: string; head: Oid; state: JobState; secrets: Record<string, string>; refs: string[] }
export interface RunPlan { head: Oid; status: "queued" | "action_required"; token: PermSet; semantics: "fork-pr" | "trusted"; jobs: JobPlan[] }

const READ_ONLY: PermSet = { contents: "read", metadata: "read" };
const ALL_PERMS: Perm[] = ["contents", "pull_requests", "issues", "statuses", "checks", "actions", "administration", "metadata"];

/** The head-bound gate rule `Merge::decide` uses (design/AGENT-LAYER.md, A3 step 9): an `AgentGate` for this exact
 *  head, from a writer who is neither the task's requester nor anyone who pushed to the PR. */
export function gateHolds(pr: Pr, head: Oid, writers: ReadonlySet<UserId>): boolean {
  const g: AgentGate | null = pr.gate;
  if (!g || g.head !== head || pr.headOid !== head) return false;
  if (!writers.has(g.satisfiedBy)) return false;
  if (pr.meta?.requester === g.satisfiedBy) return false;
  if (pr.pushers.includes(`u${g.satisfiedBy}`)) return false;
  return true;
}

/** Secret names a job references: `secrets.X`, `secrets['X']`, `secrets["X"]`, and the whole-map forms
 *  `toJSON(secrets)` and `secrets: inherit`, which reference every secret. */
export function secretRefs(job: Job, all: string[]): string[] {
  const t = job.text;
  if (/toJSON\s*\(\s*secrets\s*\)/i.test(t) || /^\s*secrets\s*:\s*inherit\b/im.test(t) || /\$\{\{[^}]*\bsecrets\s*\}\}/i.test(t)) return [...all];
  const out = new Set<string>();
  for (const m of t.matchAll(/\bsecrets\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*['"]([^'"]+)['"]\s*\])/gi)) out.add((m[1] ?? m[2]).toUpperCase());
  // `GITHUB_TOKEN` is the run's token, not a repository secret.
  // A reference to a secret the repository does not define still makes the job wait: an empty value at run time is
  // GitHub's behavior, and waiting is the safe side.
  out.delete("GITHUB_TOKEN");
  return [...out];
}

function declared(p: Job["permissions"]): PermSet {
  if (p === "write-all") return Object.fromEntries(ALL_PERMS.map((k) => [k, "write"])) as PermSet;
  return p ?? { contents: "write", metadata: "read" };
}

/** The plan for a new run. Trusted runs (no agent push, or the gate holds for this head) get the declared token and
 *  their secrets. An agent push gets fork-PR semantics: a read-only token and no secrets on every job. */
export function dispatch(wf: Workflow, trig: Trigger, policy: AgentPolicy, pr: Pr | null, writers: ReadonlySet<UserId>,
                         values: Record<string, string>): RunPlan {
  const trusted = !trig.agentPush || (pr !== null && gateHolds(pr, trig.head, writers));
  if (trusted) {
    const token = wf.jobs.reduce<PermSet>((acc, j) => ({ ...acc, ...declared(j.permissions) }), {});
    return { head: trig.head, status: "queued", token, semantics: "trusted",
             jobs: wf.jobs.map((j) => { const refs = secretRefs(j, wf.secrets);
               return { name: j.name, head: trig.head, state: { t: "Queued" }, refs, secrets: pick(values, refs) }; }) };
  }
  const approve = policy.ci === "Approve" && trig.runApprovedBy === null;
  return {
    head: trig.head, status: approve ? "action_required" : "queued", token: { ...READ_ONLY }, semantics: "fork-pr",
    jobs: wf.jobs.map((j) => {
      const refs = secretRefs(j, wf.secrets);
      const state: JobState = approve ? { t: "ActionRequired" }
        : refs.length ? { t: "Waiting", reason: `references ${refs.join(", ")}; secrets wait for graftyard/agent-gate on ${trig.head.slice(0, 7)}` }
        : { t: "Queued" };
      return { name: j.name, head: trig.head, state, refs, secrets: {} };
    }),
  };
}

/** When `AgentGate` is written, or on any later fold change, waiting jobs re-check the gate. A job is released with its
 *  secrets only if the gate holds for that job's own head now. A job whose head is no longer the PR's head stays
 *  waiting for good, superseded. */
export function release(plan: RunPlan, pr: Pr, writers: ReadonlySet<UserId>, values: Record<string, string>): RunPlan {
  const holds = gateHolds(pr, plan.head, writers);
  return { ...plan, jobs: plan.jobs.map((j) => {
    if (j.state.t !== "Waiting") return j;
    if (!holds) return pr.headOid !== j.head ? { ...j, state: { t: "Waiting", reason: `superseded by ${pr.headOid.slice(0, 7)}` } } : j;
    return { ...j, state: { t: "Queued" }, secrets: pick(values, j.refs) };
  }) };
}

const pick = (v: Record<string, string>, names: string[]): Record<string, string> =>
  Object.fromEntries(names.filter((n) => n in v).map((n) => [n, v[n]]));
