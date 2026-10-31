// M0-WP14 (L0-04), policy part only: replay the A1 agent pushes through the agent-CI dispatcher (src/wp14/ci.ts).
//
//   node run/wp14-ci.ts [--events /home/user/wp16/events.json] [--out results/raw/wp14-l0-04.json]
//
// The pushes are M0-WP16's replay: 3,134 real linux topic-branch commits pushed as agent refs (`wp16/replay.py`). Each
// ref is an agent PR with `AgentMeta` (requester alice), opened at its first push. Every push triggers each workflow in
// WORKFLOWS. Human events go into the same timeline, chosen per ref by a hash of its name: a valid approval, approvals
// that must not write a usable gate, and a human push. After each event, waiting jobs re-check the gate (`release`).
//
// The check is an oracle written apart from `gateHolds`. At every step, every job holding a secret must be on the
// PR's current head, with a gate for that head from a writer who is neither the requester nor a pusher. Any job that
// fails this is a leak. Everything is in memory, with no Actions engine. The 30 s start bound cannot be measured here.

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { createHash } from "node:crypto";
import { parseArgs } from "node:util";
import { dispatch, release, type Job, type RunPlan, type Workflow } from "../src/wp14/ci.ts";
import type { AgentPolicy, Pr } from "../src/agent/model.ts";

const ROOT = new URL("../", import.meta.url).pathname;
const { values: a } = parseArgs({ options: { events: { type: "string", default: "/home/user/wp16/events.json" },
  out: { type: "string", default: `${ROOT}results/raw/wp14-l0-04.json` }, mutant: { type: "boolean", default: false } } });
// --mutant: a broken dispatcher whose gate check ignores the head (any gate on the PR releases every waiting job). The
// oracle must report leaks for it; if it does not, a zero from the real dispatcher means nothing.
const releaseFn: typeof release = a.mutant
  ? (plan, pr, w, v) => release(plan, pr.gate ? { ...pr, headOid: plan.head, gate: { ...pr.gate, head: plan.head } } : pr, w, v)
  : release;

const U = { alice: 10, bob: 11, carol: 12, mallory: 13, dave: 14 };
const WRITERS = new Set([U.alice, U.bob, U.carol, U.dave]);
const SECRETS = { DEPLOY_KEY: "dk-7f3a91", NPM_TOKEN: "npm-1c22e0", SIGNING_KEY: "sig-55b0aa" };
const NAMES = Object.keys(SECRETS);

const job = (name: string, text: string, permissions: Job["permissions"] = null): Job => ({ name, text, permissions });
// Every way a job can name a secret that the scanner handles, plus one it cannot see (a computed index). The structural
// rule must hold for that one too.
const WORKFLOWS: Workflow[] = [
  { name: "ci", secrets: NAMES, jobs: [
    job("build", "runs-on: ubuntu-latest\nsteps:\n  - run: make test"),
    job("lint", "steps:\n  - run: make lint\n    env:\n      TOKEN: ${{ github.token }}"),
  ] },
  { name: "release", secrets: NAMES, jobs: [
    job("deploy", "steps:\n  - run: ./deploy\n    env:\n      KEY: ${{ secrets.DEPLOY_KEY }}", "write-all"),
    job("publish", "steps:\n  - run: npm publish\n    env:\n      NODE_AUTH_TOKEN: ${{ secrets['NPM_TOKEN'] }}", { contents: "write", packages: "write" } as Job["permissions"]),
    job("sign", "uses: ./.github/workflows/sign.yml\nsecrets: inherit"),
    job("dump", "steps:\n  - run: echo '${{ toJSON(secrets) }}'"),
    job("computed", "steps:\n  - run: echo ${{ secrets[format('{0}_KEY', 'SIGNING')] }}"),
  ] },
];

type Ev = { t: number; ref: string; head: string; merged: number };
const events: Ev[] = JSON.parse(readFileSync(a.events!, "utf8")).events;

function scenario(ref: string): string {
  const h = createHash("sha256").update(ref).digest()[0] / 256;
  return h < 0.40 ? "none" : h < 0.65 ? "valid" : h < 0.75 ? "requester" : h < 0.85 ? "nonwriter" : h < 0.95 ? "pusher" : "stale-head";
}

/** The oracle, written apart from `gateHolds`. */
function oracleHolds(pr: Pr, head: string): boolean {
  const g = pr.gate;
  if (!g || g.head !== head || pr.headOid !== head) return false;     // bound to this exact head, and it is current
  if (g.satisfiedBy === U.alice || g.satisfiedBy === U.mallory) return false; // the requester, or not a writer
  return !pr.pushers.includes(`u${g.satisfiedBy}`);                   // never someone who pushed to the PR
}

function runMode(ci: AgentPolicy["ci"]) {
  const policy: AgentPolicy = { ci, labels: [], extraPaths: [], extraHumanApprovals: 0, leases: "Advisory" };
  const prs = new Map<string, Pr>();
  const plans = new Map<string, RunPlan[]>();
  const pushesOf = new Map<string, number>();
  const stats = { pushes: 0, runs: 0, jobs: 0, actionRequired: 0, readOnlyRuns: 0, writeTokenAgentRuns: 0, waitingSecretJobs: 0,
    queuedNoSecretJobs: 0, queuedComputedEmpty: 0, gatesWritten: 0, gatesBad: 0, released: 0, releasedJobs: 0, superseded: 0,
    leaks: 0, leakSamples: [] as object[], decideUsMax: 0, decideUsSum: 0, byScenario: {} as Record<string, number> };
  let nextPr = 1;

  // A job is judged when it first holds secrets, at that step's fold: the row's "before AgentGate holds for its head".
  // A job released while the gate held keeps its secrets after a later push moves the head; it already started.
  // `release` returns a new object only for a job it changes, so object identity marks the jobs already judged.
  const judged = new WeakSet<object>();
  const check = (pr: Pr) => {
    for (const p of plans.get(pr.headRef) ?? []) for (const j of p.jobs) {
      if (judged.has(j)) continue;
      judged.add(j);
      if (Object.keys(j.secrets).length && !oracleHolds(pr, j.head)) {
        stats.leaks++;
        if (stats.leakSamples.length < 10) stats.leakSamples.push({ ref: pr.headRef, job: j.name, head: j.head, gate: pr.gate, cur: pr.headOid });
      }
    }
  };
  const releaseAll = (pr: Pr) => {
    for (const [i, p] of (plans.get(pr.headRef) ?? []).entries()) {
      const before = p.jobs.filter((j) => j.state.t === "Waiting").length;
      const next = releaseFn(p, pr, WRITERS, SECRETS);
      const after = next.jobs.filter((j) => j.state.t === "Waiting").length;
      if (after < before) { stats.released++; stats.releasedJobs += before - after; }
      plans.get(pr.headRef)![i] = next;
    }
    check(pr);
  };

  for (const e of events) {
    let pr = prs.get(e.ref);
    if (!pr) {
      pr = { number: nextPr++, headRef: e.ref, headOid: e.head, base: "master", draft: true, open: true, merged: false,
             meta: { app: 1, session: null, requester: U.alice, repo: "acme/mono" }, gate: null, author: "a2", pushers: [] };
      prs.set(e.ref, pr);
      plans.set(e.ref, []);
      const s = scenario(e.ref); stats.byScenario[s] = (stats.byScenario[s] ?? 0) + 1;
    }
    const prevHead = pr.headOid;
    pr.headOid = e.head;
    stats.pushes++;
    const n = (pushesOf.get(e.ref) ?? 0) + 1; pushesOf.set(e.ref, n);
    // The agent push dispatches every workflow.
    for (const wf of WORKFLOWS) {
      const t0 = process.hrtime.bigint();
      const plan = dispatch(wf, { head: e.head, agentPush: true, runApprovedBy: null }, policy, pr, WRITERS, SECRETS);
      const us = Number(process.hrtime.bigint() - t0) / 1000;
      stats.decideUsMax = Math.max(stats.decideUsMax, us); stats.decideUsSum += us;
      stats.runs++; stats.jobs += plan.jobs.length;
      if (plan.status === "action_required") stats.actionRequired++;
      const ro = Object.entries(plan.token).every(([, v]) => v === "read");
      if (ro) stats.readOnlyRuns++; else stats.writeTokenAgentRuns++;
      for (const j of plan.jobs) {
        if (j.state.t === "Waiting") stats.waitingSecretJobs++;
        if (j.state.t === "Queued" && !Object.keys(j.secrets).length) stats.queuedNoSecretJobs++;
        if (j.state.t === "Queued" && j.name === "computed") stats.queuedComputedEmpty++;
      }
      plans.get(e.ref)!.push(plan);
    }
    // Later pushes supersede: re-check every earlier plan against the new head.
    releaseAll(pr);
    // Human events, keyed to this ref's scenario, on its 1st and 3rd pushes.
    const s = scenario(e.ref);
    if (n === 1 || n === 3) {
      if (s === "valid") pr.gate = { pr: pr.number, head: pr.headOid, satisfiedBy: U.bob };
      else if (s === "requester") pr.gate = { pr: pr.number, head: pr.headOid, satisfiedBy: U.alice };
      else if (s === "nonwriter") pr.gate = { pr: pr.number, head: pr.headOid, satisfiedBy: U.mallory };
      else if (s === "pusher") { if (!pr.pushers.includes("u14")) pr.pushers.push("u14"); pr.gate = { pr: pr.number, head: pr.headOid, satisfiedBy: U.dave }; }
      else if (s === "stale-head") pr.gate = { pr: pr.number, head: prevHead, satisfiedBy: U.bob };
      if (s !== "none") { stats.gatesWritten++; if (s !== "valid") stats.gatesBad++; releaseAll(pr); }
    }
    for (const p of plans.get(e.ref)!) for (const j of p.jobs) if (j.state.t === "Waiting" && j.state.reason.startsWith("superseded")) stats.superseded++;
  }
  // Every job ever planned, at the end: every job holding secrets must be on a head whose gate holds now.
  const finalJobs = [...plans.values()].flat().flatMap((p) => p.jobs);
  return { ...stats, decideUsMean: +(stats.decideUsSum / stats.runs).toFixed(2), prs: prs.size,
           finalWithSecrets: finalJobs.filter((j) => Object.keys(j.secrets).length).length,
           finalWaiting: finalJobs.filter((j) => j.state.t === "Waiting").length };
}

const out = { at: new Date().toISOString(), events: events.length, workflows: WORKFLOWS.map((w) => ({ name: w.name, jobs: w.jobs.map((j) => j.name) })),
              AutoReadOnly: runMode("AutoReadOnly"), Approve: runMode("Approve") };
mkdirSync(`${ROOT}results/raw`, { recursive: true });
if (!a.mutant) writeFileSync(a.out!, JSON.stringify(out, null, 1));
const brief = (m: any) => ({ ...m, leakSamples: m.leakSamples.length, byScenario: m.byScenario });
console.log(JSON.stringify({ events: out.events, AutoReadOnly: brief(out.AutoReadOnly), Approve: brief(out.Approve) }, null, 1));
