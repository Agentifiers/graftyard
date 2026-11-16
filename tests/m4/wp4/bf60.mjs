// BF-60 on graft-m4a with `graft-runners` in the runner account (build/BUILD-PLAN.md, M4-WP4): 50 `ubuntu-latest`
// jobs queued for one owner with the cap at 20, while `graft-runners`' 5-minute sweeps run. Pass: at most 20 hosted
// runners at once; every job finishes; no Sandbox is left 10 minutes after the last job. Fail: more than 20 at once, a
// job that never finishes, or a leaked Sandbox.
//
// The install delivers no `workflow_job` to Apps yet (decisions/OPEN-QUESTIONS.md, R24), so `graft-runners` runs on
// its sweep alone, the design's own path for a lost webhook. Nothing here prompts a pass. Each job's queue-to-start
// time, from this driver's 5 s polls of the run, shows what the sweep costs.
//
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp4/bf60.mjs <out dir>        (RUNNER_ACCOUNT and CLOUDFLARE_RUNNER_API_TOKEN for the Sandbox count)
//
// Observed every 10 s: the runner account's running Sandbox instances (Containers API, both applications), and the
// owner's hosted demand (registered hosted runners, through the App's installation token) every `DEMAND_EVERY_S`
// (default 10). A demand read reaches the owner's `RunnerBroker`, so reads every 10 s keep it in memory; with
// `DEMAND_EVERY_S=300` the driver reads demand only as often as the sweep, and the broker can leave memory between
// reads, which tests its stored `Seen`. After the last job, it keeps observing for 10 minutes and then reads both once
// more.
import { createSign } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Git, api, log, row, secret, sleep, tokens } from "../../am1/hosted/lib.mjs";

const out = resolve(process.argv[2] ?? "tests/m4/raw/wp4/bf60");
mkdirSync(out, { recursive: true });
const NAME = process.env.BF60_REPO ?? "wp4-bf60";
const R = `m3org/${NAME}`;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const H = `${process.env.HOME}/.graft-m4a/hosted`;
const app = JSON.parse(readFileSync(`${H}/app.json`, "utf8"));
const PEM = readFileSync(`${H}/app.pem`, "utf8");
const ACCT = process.env.RUNNER_ACCOUNT ?? "<cf-runner-account-id>";
const CF = process.env.CLOUDFLARE_RUNNER_API_TOKEN;
if (!CF) throw new Error("CLOUDFLARE_RUNNER_API_TOKEN is not set");
secret(CF);
const samples = join(out, "samples.jsonl");
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};

// ---- the App's installation token, renewed every 30 minutes
let ghs = null, ghsAt = 0;
async function appToken() {
  if (ghs && Date.now() - ghsAt < 30 * 60_000) return ghs;
  const b = (x) => Buffer.from(x).toString("base64url"), now = Math.floor(Date.now() / 1000);
  const h = b(JSON.stringify({ alg: "RS256", typ: "JWT" })), p = b(JSON.stringify({ iat: now - 30, exp: now + 540, iss: String(app.app) }));
  const jwt = secret(`${h}.${p}.${b(createSign("RSA-SHA256").update(`${h}.${p}`).sign(PEM))}`);
  const r = await api("POST", `/api/v3/app/installations/${app.installation}/access_tokens`, null, {}, { authorization: `Bearer ${jwt}` });
  ghs = secret(must("access_tokens", r, 201).token);
  ghsAt = Date.now();
  return ghs;
}
async function demand() {
  const r = await api("GET", "/api/graftyard/v1/owners/m3org/actions/hosted/demand", await appToken());
  return r.status === 200 ? r.body : { error: r.status };
}
async function cf(path) {
  const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${ACCT}${path}`, { headers: { authorization: `Bearer ${CF}` }, signal: AbortSignal.timeout(30_000) });
  return r.json();
}
async function sandboxes() {
  const apps = (await cf("/containers/applications")).result ?? [];
  let running = 0, all = 0;
  const states = {};
  for (const a of apps.filter((x) => x.name.startsWith("graft-runners-"))) {
    const v = await cf(`/containers/applications/${a.id}/instances?per_page=1000`);
    for (const i of v.result?.instances ?? []) {
      all += 1;
      // The instances list answers `status.state`; a stopped Sandbox stays listed `inactive` (smoke run).
      const st = i.status?.state ?? "unknown";
      states[st] = (states[st] ?? 0) + 1;
      if (!["inactive", "stopped", "failed", "stopping", "deleted"].includes(st)) running += 1;
    }
  }
  return { running, all, states };
}

// ---- setup: the repository and workflow
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bf60.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp4/hosted/bf60.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const A = `/api/graftyard/v1/repos/${R}/actions`;
const before = { demand: await demand(), sandboxes: await sandboxes() };
log(`repo ${R} (repo:${repoId}); before: ${JSON.stringify(before.demand).slice(0, 200)}; sandboxes ${JSON.stringify(before.sandboxes)}`);

// ---- the run
const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" }), 201).run;
const t0 = Date.now();
log(`run ${run} dispatched`);
const jobs = new Map(); // id -> {queued, started, done, conclusion}
const DEMAND_EVERY = Number(process.env.DEMAND_EVERY_S ?? 10) * 1000;
let maxRegistered = 0, maxSandboxes = 0, lastDone = null, nextSample = 0, nextDemand = 0, d = {};
const result = { repo: R, repoId, run, before, demandEveryS: DEMAND_EVERY / 1000 };
for (;;) {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  const now = Date.now();
  if (r.status === 200) {
    for (const j of r.body.value.jobs) {
      const st = Object.keys(j.state)[0];
      const e = jobs.get(j.spec.id) ?? {};
      if (st === "Queued" && !e.queued) e.queued = now;
      if ((st === "Assigned" || st === "InProgress" || st === "Completed") && !e.started) e.started = now;
      if (st === "Completed" && !e.done) { e.done = now; e.conclusion = j.state.Completed.conclusion; }
      jobs.set(j.spec.id, e);
    }
  }
  if (now >= nextSample) {
    nextSample = now + 10_000;
    const fresh = now >= nextDemand;
    if (fresh) { nextDemand = now + DEMAND_EVERY; d = await demand(); }
    const s = await sandboxes();
    const registered = fresh ? d.runners?.length ?? null : null;
    maxRegistered = Math.max(maxRegistered, registered ?? 0);
    maxSandboxes = Math.max(maxSandboxes, s.running);
    const counts = {};
    for (const e of jobs.values()) { const k = e.done ? "done" : e.started ? "started" : "queued"; counts[k] = (counts[k] ?? 0) + 1; }
    row(samples, { t: now, s: Math.round((now - t0) / 1000), registered, ...(fresh ? { stale: d.stale?.length ?? null, starts: d.labels?.map(([l, x]) => [l, x.queued, x.start]) ?? d } : {}), sandboxes: s, counts });
    log(`+${Math.round((now - t0) / 1000)}s jobs ${JSON.stringify(counts)} registered ${registered} sandboxes ${s.running} (${JSON.stringify(s.states)})`);
  }
  const done = [...jobs.values()].filter((e) => e.done).length;
  if (jobs.size === 50 && done === 50) { lastDone = Math.max(...[...jobs.values()].map((e) => e.done)); break; }
  if (now - t0 > (Number(process.env.GIVE_UP_MIN ?? 120)) * 60_000) { log("gave up"); break; }
  await sleep(5000);
}

// ---- 10 minutes after the last job: no Sandbox and no hosted registration left
if (lastDone) {
  log("every job done; watching 10 minutes for leftover Sandboxes");
  while (Date.now() < lastDone + 10 * 60_000) {
    const fresh = Date.now() >= nextDemand;
    if (fresh) { nextDemand = Date.now() + DEMAND_EVERY; d = await demand(); }
    const s = await sandboxes();
    row(samples, { t: Date.now(), after: true, registered: fresh ? d.runners?.length ?? null : null, sandboxes: s });
    maxSandboxes = Math.max(maxSandboxes, s.running);
    await sleep(30_000);
  }
}
const after = { at: Date.now(), demand: await demand(), sandboxes: await sandboxes() };
const lat = [...jobs.values()].filter((e) => e.queued && e.started).map((e) => (e.started - e.queued) / 1000).sort((a, b) => a - b);
const q = (p) => lat.length ? lat[Math.min(lat.length - 1, Math.floor(p * lat.length))] : null;
const checks = {
  jobs50: jobs.size === 50,
  atMost20RegisteredAtOnce: maxRegistered <= 20,
  atMost20SandboxesAtOnce: maxSandboxes <= 20,
  everyJobFinished: jobs.size === 50 && [...jobs.values()].every((e) => e.done),
  everyJobSucceeded: [...jobs.values()].every((e) => e.conclusion === "Success"),
  noSandboxLeft10MinAfter: !!lastDone && after.sandboxes.running === 0,
  noRegistrationLeft10MinAfter: !!lastDone && (after.demand.runners?.length ?? 1) === 0,
};
Object.assign(result, {
  lastJobS: lastDone ? Math.round((lastDone - t0) / 1000) : null, maxRegistered, maxSandboxes, after,
  queueToStartS: { n: lat.length, p50: q(0.5), p90: q(0.9), max: lat.at(-1) ?? null },
  jobs: Object.fromEntries([...jobs].map(([k, e]) => [k, { queuedS: e.queued && (e.queued - t0) / 1000, startedS: e.started && (e.started - t0) / 1000, doneS: e.done && (e.done - t0) / 1000, conclusion: e.conclusion }])),
  checks, pass: Object.values(checks).every(Boolean),
});
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
log(`BF-60: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)}; last job ${result.lastJobS} s; max registered ${maxRegistered}, max Sandboxes ${maxSandboxes}; queue to start ${JSON.stringify(result.queueToStartS)}`);
process.exit(result.pass ? 0 : 1);
