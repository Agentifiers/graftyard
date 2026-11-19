// R23's cold-burst check on graft-m4a with `graft-runners` in the runner account (decisions/DECISIONS.md,
// `r23-runnerbox`): one run of 8 hosted jobs (`tests/m4/wp4/hosted/r23.yml`: 6 `ubuntu-22.04` on `standard-3`, 2
// `ubuntu-24.04-4core` on `standard-4`), started at once by the `workflow_job` webhooks, while the deployed
// `graft-runners` version sets `RUNNER_BOX_ON=1`, so every start is a `RunnerBox` on the `durable_object` application.
// Each job prints PID 1's argv[0..1] and comm, whether the Sandbox SDK's server runs, and its vCPU and memory.
//
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp4/r23.mjs <out dir>        (RUNNER_ACCOUNT and CLOUDFLARE_RUNNER_API_TOKEN for the instance count)
//
// Every 10 s: the run's jobs and the `RunnerBox` application's instances. After the last job, 10 more minutes, then
// once more. Pass: 8 jobs Success; PID 1 is the runner on each; no instance and no hosted registration left.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { EDGE, Git, api, log, row, secret, sleep, tokens } from "../../am1/hosted/lib.mjs";

const out = resolve(process.argv[2] ?? "tests/m4/raw/wp4/r23");
mkdirSync(out, { recursive: true });
const NAME = "wp4-r23";
const R = `m3org/${NAME}`;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const ACCT = process.env.RUNNER_ACCOUNT ?? process.env.CLOUDFLARE_RUNNER_ACCOUNT_ID;
const CF = secret(process.env.CLOUDFLARE_RUNNER_API_TOKEN);
const samples = join(out, "samples.jsonl");
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};
async function cf(path) {
  const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${ACCT}${path}`, { headers: { authorization: `Bearer ${CF}` }, signal: AbortSignal.timeout(30_000) });
  return r.json();
}
async function boxes() {
  const apps = (await cf("/containers/applications")).result ?? [];
  const out = {};
  for (const a of apps.filter((x) => x.name.startsWith("graft-runners-"))) {
    const v = await cf(`/containers/applications/${a.id}/instances?per_page=1000`);
    const states = {};
    for (const i of v.result?.instances ?? []) {
      const st = i.status?.state ?? "unknown";
      states[st] = (states[st] ?? 0) + 1;
    }
    out[a.name.replace("graft-runners-", "")] = states;
  }
  return out;
}
const live = (b) => Object.values(b).flatMap((s) => Object.entries(s)).filter(([k]) => !["inactive", "stopped", "failed", "stopping", "deleted"].includes(k)).reduce((n, [, c]) => n + c, 0);

const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/r23.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp4/hosted/r23.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const A = `/api/graftyard/v1/repos/${R}/actions`;
log(`repo ${R} (repo:${repoId}); boxes before ${JSON.stringify(await boxes())}`);

const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" }), 201).run;
const t0 = Date.now();
log(`run ${run} dispatched`);
const jobs = new Map();
let maxLive = 0, lastDone = null, nextSample = 0, specs = [];
for (;;) {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  const now = Date.now();
  if (r.status === 200) {
    specs = r.body.value.jobs;
    for (const j of specs) {
      const st = Object.keys(j.state)[0];
      const e = jobs.get(j.spec.id) ?? {};
      if (!e.queued) e.queued = now;
      if (st !== "Queued" && !e.started) e.started = now;
      if (st === "Completed" && !e.done) { e.done = now; e.conclusion = j.state.Completed.conclusion; }
      jobs.set(j.spec.id, e);
    }
  }
  if (now >= nextSample) {
    nextSample = now + 10_000;
    const b = await boxes();
    maxLive = Math.max(maxLive, live(b));
    const counts = {};
    for (const e of jobs.values()) { const k = e.done ? "done" : e.started ? "started" : "queued"; counts[k] = (counts[k] ?? 0) + 1; }
    row(samples, { t: now, s: Math.round((now - t0) / 1000), boxes: b, counts });
    log(`+${Math.round((now - t0) / 1000)}s jobs ${JSON.stringify(counts)} boxes ${JSON.stringify(b)}`);
  }
  const done = [...jobs.values()].filter((e) => e.done).length;
  if (jobs.size === 8 && done === 8) { lastDone = Math.max(...[...jobs.values()].map((e) => e.done)); break; }
  if (now - t0 > Number(process.env.GIVE_UP_MIN ?? 20) * 60_000) { log("gave up"); break; }
  await sleep(5000);
}

const legs = [];
for (const j of specs) {
  let text = "";
  for (let i = 0; i < 6; i++) {
    const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${j.spec.id}/logs`, { headers: { authorization: `token ${admin}` } });
    text = await r.text();
    if (/Z r23 nproc /m.test(text)) break;
    await sleep(5000);
  }
  // Output lines only: the step's echoed script holds the same words.
  const pick = (k) => text.match(new RegExp(`Z r23 ${k} (.*)$`, "m"))?.[1]?.trim() ?? null;
  const leg = { job: j.spec.id, name: j.spec.name, conclusion: j.state.Completed?.conclusion ?? Object.keys(j.state)[0],
    pid1: pick("pid1"), comm1: pick("comm1"), sandboxServer: pick("sandbox-server"), size: pick("nproc") };
  legs.push(leg);
  log(`job ${leg.job} ${leg.name}: ${leg.conclusion}; pid1 ${leg.pid1}; comm ${leg.comm1}; sandbox server ${leg.sandboxServer}; ${leg.size}`);
}

if (lastDone) {
  log("every job done; watching 10 minutes for leftover boxes");
  while (Date.now() < lastDone + 10 * 60_000) {
    const b = await boxes();
    row(samples, { t: Date.now(), after: true, boxes: b });
    await sleep(30_000);
  }
}
const after = await boxes();
const checks = {
  jobs8: jobs.size === 8,
  everyJobSucceeded: jobs.size === 8 && legs.every((l) => l.conclusion === "Success"),
  // `exec run.sh`: PID 1 is the shell running run.sh, never the Sandbox SDK's server.
  runnerIsPid1: legs.length === 8 && legs.every((l) => /run\.sh|Runner\.Listener/.test(l.pid1 ?? "") && !/container-server|sandbox/.test(l.pid1 ?? "")),
  noBoxLeft10MinAfter: !!lastDone && live(after) === 0,
};
const result = { repo: R, repoId, run, lastJobS: lastDone ? Math.round((lastDone - t0) / 1000) : null, maxLive, after, legs,
  jobs: Object.fromEntries([...jobs].map(([k, e]) => [k, { startedS: e.started && (e.started - t0) / 1000, doneS: e.done && (e.done - t0) / 1000, conclusion: e.conclusion }])),
  checks, pass: Object.values(checks).every(Boolean) };
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
log(`R23 cold burst: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)}; last job ${result.lastJobS} s; at most ${maxLive} live instances`);
process.exit(result.pass ? 0 : 1);
