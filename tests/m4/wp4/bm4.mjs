// BM-4 on graft-m4a with hosted runners (build/BUILD-PLAN.md, Milestone 4; M4-WP4 carries its matrix run): an
// unmodified `actions/runner` on `graft-runners`' Sandboxes runs a 2 x 4 matrix (`tests/m4/wp4/hosted/bm4.yml`, legs
// of about 5 minutes), and a site is failed mid-run. Pass: no lost job state, so every leg ends `Success`, the run ends
// `Success`, and no job is ever held by two runners. Fail: a leg that fails, is cancelled, or never ends, or a job
// that moves to a second runner.
//
//   source tests/am1/lf13/env-m4a.sh
//   KILL="DEPLOY_TREE=/tmp/m4a-deploy IMG=<image> bash tests/m4/wp3/bf55-kill.sh down" \
//   RESTORE="DEPLOY_TREE=/tmp/m4a-deploy IMG=<image> bash tests/m4/wp3/bf55-kill.sh up" \
//   node tests/m4/wp4/bm4.mjs <out dir>
//
// The site failed is the leader site of the repository's Actions shard, where the jobs' state lives, read when every
// leg is running (Assigned or InProgress); the kill command gets that site's id, as BF-55's does. `RESTORE` brings it
// back `RESTORE_AFTER_S` (180) after its old leader stops answering. `graft-runners` runs on its sweep alone (R24).
// Observed every 5 s: each job's state and the runner holding it.
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Git, api as api1, log, member, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const out = resolve(process.argv[2] ?? "tests/m4/raw/wp4/bm4");
mkdirSync(out, { recursive: true });
const NAME = process.env.BM4_REPO ?? "wp4-bm4";
const R = `m3org/${NAME}`;
const ORG = 7100001;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const samples = join(out, "samples.jsonl");
async function api(...a) {
  for (let i = 0; ; i++) {
    const r = await api1(...a);
    if (r.status !== 503 || i >= 5) return r;
    await sleep(3000);
  }
}
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};
async function statusOf(chain, site) {
  try {
    const r = await member(chain, site, "/status", undefined, 15_000);
    return r.status === 200 && r.v && typeof r.v === "object" ? r.v : null;
  } catch { return null; }
}
async function leader(chain) {
  const st = (await Promise.all([0, 1, 2].map((s) => statusOf(chain, s)))).filter(Boolean);
  const l = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  return l ? { site: l.site, term: l.term } : null;
}
function actionsShard(repoId) {
  const b = Buffer.alloc(8);
  b.writeBigInt64BE(BigInt(repoId));
  return createHash("sha256").update(b).digest()[0] % 16;
}

// ---- setup
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const jobChain = `actions:${ORG}.${actionsShard(repoId)}`;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bm4.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp4/hosted/bm4.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const A = `/api/graftyard/v1/repos/${R}/actions`;
const result = { repo: R, repoId, jobChain, events: [] };
const event = (what, extra = {}) => { const e = { at: Date.now(), what, ...extra }; result.events.push(e); log(`${what} ${JSON.stringify(extra)}`); };
event("setup", { jobChain, regChain: `actions:${ORG}.0` });

// ---- the run, the kill, the restore
const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" }), 201).run;
event("run created", { run });
const holders = {}; // job id -> runner ids seen holding it
let killed = null, dead = null, killSite = null, killProc = null, restoreProc = null, jobs = [];
async function watchDead() {
  while (!dead) {
    const st = await statusOf(jobChain, killSite);
    if (!st || st.joined === false || st.role !== "leader") {
      dead = Date.now();
      event("old leader dead", { site: killSite, status: st && { role: st.role, term: st.term } });
      break;
    }
    await sleep(1000);
  }
  if (!process.env.RESTORE) return;
  await sleep(Number(process.env.RESTORE_AFTER_S ?? 180) * 1000);
  event("restore", { site: killSite });
  restoreProc = spawn("bash", ["-c", `${process.env.RESTORE} ${killSite}`], { stdio: ["ignore", "inherit", "inherit"] });
  restoreProc.on("exit", (code) => event("restore command done", { code }));
}
const t0 = Date.now();
for (;;) {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  if (r.status === 200) {
    jobs = r.body.value.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state, version: j.version }));
    const counts = {};
    for (const j of jobs) {
      counts[j.state] = (counts[j.state] ?? 0) + 1;
      const runner = j.s.Assigned?.runner ?? j.s.InProgress?.runner;
      if (runner != null) (holders[j.id] ??= []).includes(runner) || holders[j.id].push(runner);
    }
    row(samples, { t: Date.now(), s: Math.round((Date.now() - t0) / 1000), counts, dead: !!dead });
    const running = (counts.Assigned ?? 0) + (counts.InProgress ?? 0);
    if (!killed && running === jobs.length && jobs.length) {
      killSite = (await leader(jobChain))?.site;
      const reg = await leader(`actions:${ORG}.0`);
      killed = Date.now();
      event("kill", { site: killSite, registrationLeader: reg, counts, cmd: process.env.KILL });
      if (process.env.KILL && killSite != null) {
        killProc = spawn("bash", ["-c", `${process.env.KILL} ${killSite}`], { stdio: ["ignore", "inherit", "inherit"] });
        killProc.on("exit", (code) => event("kill command done", { code }));
        watchDead();
      }
    }
    if (jobs.length && (counts.Completed ?? 0) === jobs.length) break;
  }
  if (Date.now() - t0 > Number(process.env.GIVE_UP_MIN ?? 60) * 60_000) { event("gave up"); break; }
  await sleep(5000);
}
// The restore is due `RESTORE_AFTER_S` after `dead`: wait for `watchDead` to start it, then for it to end. (Run 1900546
// ended its legs before the timer and exited with site a still down, which a site day 45 by hand brought back.)
while (dead && process.env.RESTORE && !restoreProc) await sleep(1000);
for (const p of [killProc, restoreProc]) if (p && p.exitCode === null) await new Promise((r) => p.on("exit", r));
if (killed && process.env.RESTORE && !restoreProc) event("restore never ran: the old leader never went dead");

// ---- the verdict
const runStatus = must("run", await api("GET", `/api/v3/repos/${R}/actions/runs/${run}`, admin));
const twice = Object.entries(holders).filter(([, rs]) => rs.length > 1).map(([id, rs]) => ({ id, runners: rs }));
const checks = {
  jobs8: jobs.length === 8,
  siteFailedMidRun: !!dead,
  everyLegSucceeded: jobs.length === 8 && jobs.every((j) => j.s.Completed?.conclusion === "Success"),
  runSucceeded: runStatus.conclusion === "success",
  noJobOnTwoRunners: twice.length === 0,
};
Object.assign(result, {
  run, killSite, killed, dead, runConclusion: runStatus.conclusion, twice, holders,
  jobs: jobs.map((j) => ({ id: j.id, name: j.name, version: j.version, state: j.s })), checks, pass: Object.values(checks).every(Boolean),
});
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
log(`BM-4: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
process.exit(result.pass ? 0 : 1);
