// BF-55 on a hosted install (`build/BUILD-PLAN.md`, M4-WP3): kill the leader site of an `Owner` scope's registration
// shard during a 256-job matrix. Pass: polls reach a broker on the new leader site within 15 s; every job runs once;
// zero jobs assigned twice. Fail: a job assigned twice, or dispatch stalled over 60 s.
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp3/bf55.mjs <out dir> <runner dir>...      (KILL=<cmd> runs the kill; see below)
// Setup: `m3org/wp3-bf55` with `tests/m4/wp3/hosted/bf55.yml` (a 16 x 16 matrix, 256 jobs), and one `Owner`-scope
// ephemeral runner per runner dir at a time, registered on the owner's shard 0 (`"scope": "owner"`), each re-registered
// after its job. The kill: once a quarter of the matrix finished, the driver runs `$KILL` (the site-a down day,
// `tests/m4/wp3/bf55-kill.sh down`), which takes the site the registration shard leads on out, and marks its end.
// Observed, on Cloudflare's clock (members' `/status` `now`):
//   - the brokers' `/stats` on every site through `graft-m4a-memberkill`'s `BROKER_<n>` bindings, every 2 s: the first
//     poll a broker on another site takes after the kill;
//   - the run's jobs every 2 s, for the longest gap with no job leaving `Queued`;
//   - when the registration shard's old leader stops answering (`/status` every second from the kill), the kill's time;
//   - after the run, each job's version: `Queued` v0, `Matched` v1, `Acquired` v2, `Finished` v3, so a job that ends at
//     version 3 was matched once and acquired once, and a second assignment (a lease that requeued it, or a second
//     match) ends it higher. A 3 s job never rewrites its lease (`LEASE_REWRITE_UNDER`).
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Git, api as api1, log, member, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const [outArg, ...runnerDirs] = process.argv.slice(2);
const out = resolve(outArg);
const NAME = process.env.BF55_REPO ?? "wp3-bf55";
const R = `m3org/${NAME}`;
const ORG = 7100001;
const admin = tokens.admin;
const KILL = process.env.MEMBERKILL ?? "https://graft-m4a-memberkill.<workers-subdomain>.workers.dev";
const KILL_TOKEN = readFileSync(`${process.env.HOME}/.graft-m4a/memberkill-token`, "utf8").trim();
const repoRoot = new URL("../../..", import.meta.url).pathname;
const jit = join(repoRoot, "target/release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const polls = join(out, "polls.jsonl");
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
  try { return (await member(chain, site, "/status", undefined, 15_000)).v; } catch { return null; }
}
async function leader(chain) {
  const st = (await Promise.all([0, 1, 2].map((s) => statusOf(chain, s)))).filter(Boolean);
  const l = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  return l ? { site: l.site, term: l.term } : null;
}
/** A broker object's `/stats` on `site`, through the scratch Worker's `BROKER_<site>` binding. */
async function brokerStats(site, name) {
  try {
    const r = await fetch(`${KILL}/?site=${site}&name=${encodeURIComponent(name)}&path=/stats&class=BROKER`, {
      method: "POST", headers: { "x-t": KILL_TOKEN }, body: "{}", signal: AbortSignal.timeout(10_000),
    });
    const v = await r.json();
    return v.status === 200 ? JSON.parse(v.body) : null;
  } catch { return null; }
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
const shard = actionsShard(repoId);
const jobChain = `actions:${ORG}.${shard}`;
const regChain = `actions:${ORG}.0`;
if (shard === 0) throw new Error("the repository hashes to shard 0; pick another name so the jobs' shard is not the registration shard");
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bf55.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp3/hosted/bf55.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const A = `/api/graftyard/v1/repos/${R}/actions`;
let offset = 0;
{
  const xs = [];
  for (let i = 0; i < 3; i++) {
    const t0 = Date.now();
    const s = await statusOf(`repo:${repoId}`, i);
    if (s?.now) xs.push(s.now - (t0 + Date.now()) / 2);
  }
  xs.sort((a, b) => a - b);
  offset = xs[1] ?? xs[0] ?? 0;
}
const cf = () => Date.now() + offset;
const result = { repo: R, repoId, jobChain, regChain, offsetMs: Math.round(offset), events: [] };
const event = (what, extra = {}) => { const e = { at: cf(), what, ...extra }; result.events.push(e); log(`${what} ${JSON.stringify(extra)}`); };
log(`repo ${R} (repo:${repoId}), jobs on ${jobChain}, runners on ${regChain}; Cloudflare ${Math.round(offset)} ms ahead`);

// ---- runners: Owner scope, ephemeral, one per dir at a time
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
async function startRunner(slot) {
  n += 1;
  const dir = join(out, `runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf55-${n}`, scope: "owner" });
  if (reg.status !== 201) { log(`register ${n}: ${reg.status} ${JSON.stringify(reg.body).slice(0, 200)}`); return; }
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg.body));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf55-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slot.dir, f), { force: true });
  rmSync(join(slot.dir, "_diag"), { recursive: true, force: true });
  rmSync(join(slot.dir, "_work"), { recursive: true, force: true });
  const o = join(out, `runner-${n}.out`);
  slot.child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`], { cwd: slot.dir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  slot.child.stdin.end(config);
  slot.started = Date.now();
}

// The registration shard exists once the first runner registers; its leader site is the kill's target.
await startRunner(slots[0]);
let regLeader = null;
for (let i = 0; i < 30 && !regLeader; i++) { regLeader = await leader(regChain); if (!regLeader) await sleep(1000); }
if (!regLeader) throw new Error(`no leader for ${regChain}`);
event("registration shard leader", { regLeader });
for (const slot of slots.slice(1)) await startRunner(slot);

const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" }), 201).run;
event("run created", { run });
const jobsOf = async () => {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  if (r.status !== 200) return null;
  return r.body.value.jobs.map((j) => ({ id: j.spec.id, state: Object.keys(j.state)[0], s: j.state, version: j.version }));
};

// ---- the run, the kill, and the polls
const brokerName = `broker:${regChain}`;
const statsBefore = {};
let killed = null, killDone = null, dead = null, firstNewPoll = null, lastQueuedChange = cf(), longestStall = 0, lastQueued = null;
const gaps = [];
let killProc = null;
// The kill runs beside the loop; `dead` is when the old leader's member first fails to answer.
async function watchDead() {
  while (!dead) {
    const st = await statusOf(regChain, regLeader.site);
    if (!st || st.joined === false) { dead = cf(); event("old leader dead", { site: regLeader.site }); return; }
    await sleep(1000);
  }
}
const t0 = Date.now();
for (;;) {
  const jobs = await jobsOf();
  const now = cf();
  if (jobs) {
    const counts = {};
    for (const j of jobs) counts[j.state] = (counts[j.state] ?? 0) + 1;
    const queued = counts.Queued ?? 0;
    if (lastQueued !== null && queued !== lastQueued) {
      if (now - lastQueuedChange > 20_000) gaps.push({ from: lastQueuedChange, to: now, ms: now - lastQueuedChange });
      longestStall = Math.max(longestStall, now - lastQueuedChange);
      lastQueuedChange = now;
    }
    if (queued === 0) lastQueuedChange = now;
    lastQueued = queued;
    const stats = await Promise.all([0, 1, 2].map((s) => brokerStats(s, brokerName)));
    row(polls, { t: now, counts, stats, dead: !!dead });
    if (!killed) for (const s of [0, 1, 2]) statsBefore[s] = stats[s];
    if (dead && !firstNewPoll) {
      for (const s of [0, 1, 2]) {
        if (s === regLeader.site || !stats[s]) continue;
        const was = statsBefore[s]?.polls ?? 0;
        if (stats[s].polls > was) {
          firstNewPoll = { site: s, at: stats[s].lastPoll, polls: stats[s].polls - was, newLeader: await leader(regChain) };
          event("first poll on a new site", firstNewPoll);
        }
      }
    }
    const done = counts.Completed ?? 0;
    if (!killed && done >= 64) {
      killed = cf();
      event("kill", { regLeader, done, cmd: process.env.KILL });
      if (process.env.KILL) {
        killProc = spawn("bash", ["-c", `${process.env.KILL} ${regLeader.site}`], { stdio: ["ignore", "inherit", "inherit"] });
        killProc.on("exit", (code) => { killDone = cf(); event("kill command done", { code }); });
        watchDead();
      }
    }
    if (done === jobs.length && jobs.length) break;
    if (dead && now - lastQueuedChange > 15 * 60_000) { event("stalled 15 min"); break; }
  }
  for (const slot of slots) {
    const busy = slot.child && slot.child.exitCode === null;
    if (!busy && jobs?.some((j) => j.state === "Queued")) await startRunner(slot);
  }
  if (Date.now() - t0 > 90 * 60_000) { event("gave up after 90 min"); break; }
  await sleep(2000);
}
for (const slot of slots) if (slot.child && slot.child.exitCode === null) try { process.kill(-slot.child.pid, "SIGKILL"); } catch {}

// ---- the verdict
const jobs = (await jobsOf()) ?? [];
const counts = {};
for (const j of jobs) counts[j.s.Completed?.conclusion ?? j.state] = (counts[j.s.Completed?.conclusion ?? j.state] ?? 0) + 1;
const versions = {};
for (const j of jobs) versions[j.version] = (versions[j.version] ?? 0) + 1;
const twice = jobs.filter((j) => j.version !== 3).map((j) => ({ id: j.id, version: j.version, state: j.s }));
const checks = {
  jobs256: jobs.length === 256,
  killed: !!dead,
  everyJobSucceeded: jobs.every((j) => j.s.Completed?.conclusion === "Success"),
  firstPollOnNewSiteWithin15s: !!firstNewPoll && !!dead && firstNewPoll.at - dead <= 15_000,
  everyJobMatchedAndAcquiredOnce: twice.length === 0,
  noStallOver60s: longestStall <= 60_000,
};
Object.assign(result, { run, killed, dead, killDone, firstNewPoll, firstNewPollAfterDeadMs: firstNewPoll && dead ? firstNewPoll.at - dead : null,
  longestStallMs: longestStall, gaps, counts, versions, twice, runnersStarted: n, checks, pass: Object.values(checks).every(Boolean) });
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
log(`BF-55: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
log(`  first new-site poll ${result.firstNewPollAfterDeadMs ?? "none"} ms after the old leader went dead; longest stall ${longestStall} ms; ${n} runners; ${JSON.stringify(counts)}; versions ${JSON.stringify(versions)}`);
if (killProc && killProc.exitCode === null) await new Promise((r) => killProc.on("exit", r));
process.exit(result.pass ? 0 : 1);
