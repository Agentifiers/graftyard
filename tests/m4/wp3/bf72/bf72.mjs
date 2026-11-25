// BF-72 on graft-m4a (build/BUILD-PLAN.md, M4-WP3; decisions/DECISIONS.md, "`reusable`"; plan in ./plan.md): an
// unmodified actions/runner 2.338.2 runs tests/m4/wp3/bf72/caller.yml, whose job `uses:` m3org/bf72-shared's
// deploy.yml at tag v1, with `with:` inputs, `secrets: inherit`, an output a later job reads, and an ID token whose
// `job_workflow_ref` names the called file. The local row is ./run.sh; this checks the same eight lines from job logs.
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf72/bf72.mjs run <out> <runner dir>
// `run` writes one JSON line into <out>/result.jsonl: every job's name and conclusion and the BF72 lines it printed.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { EDGE, Git, api, log, row, secret, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [what, outArg, ...runnerDirs] = process.argv.slice(2);
if (what !== "run" || !outArg || !runnerDirs.length) throw new Error("usage: bf72.mjs run <out> <runner dir>...");
const out = resolve(outArg);
const CALLER = process.env.BF72_REPO ?? "bf72", SHARED = process.env.BF72_SHARED ?? "bf72-shared";
const R = `m3org/${CALLER}`, A = `/api/graftyard/v1/repos/${R}/actions`, WF = ".github/workflows/caller.yml";
const admin = tokens.admin;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const must = (w, r, ...ok) => { if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`); return r.body; };
const t0 = new Date();

async function repo(name) {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: false, auto_init: true });
  if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must(`create ${name}`, made, 201);
  const g = new Git(`m3org/${name}`).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error(`clone ${name}`); await sleep(3000); }
  return g;
}
const push = (g, ...refs) => { for (const r of refs) if (!g.run(["push", "-q", "-f", "origin", r]).ok) throw new Error(`push ${r}`); };

// ---- m3org/bf72-shared: deploy.yml at tag v1, and a branch also named v1 whose file prints /branch (GHES: the tag wins)
const s = await repo(SHARED);
const called = readFileSync(new URL("./shared/.github/workflows/deploy.yml", import.meta.url), "utf8");
mkdirSync(join(s.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(s.wt, ".github/workflows/deploy.yml"), called);
s.run(["add", "-A"]); s.run(["commit", "-q", "--allow-empty", "-m", "BF-72 v1"]);
s.run(["tag", "-f", "v1"]);
push(s, "HEAD:refs/heads/main", "refs/tags/v1:refs/tags/v1");
s.run(["checkout", "-q", "-B", "branch-v1"]);
writeFileSync(join(s.wt, ".github/workflows/deploy.yml"), called.replace('/v1"', '/branch"'));
s.run(["commit", "-q", "-am", "BF-72 decoy branch"]);
push(s, "HEAD:refs/heads/v1");

// ---- m3org/bf72: caller.yml with `uses:` pointed at m3org/bf72-shared, and the repository secret DEPLOY_KEY
const g = await repo(CALLER);
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, WF), readFileSync(new URL("./caller.yml", import.meta.url), "utf8").replaceAll("octo/shared/", `m3org/${SHARED}/`));
g.run(["add", "-A"]); g.run(["commit", "-q", "--allow-empty", "-m", "BF-72"]);
push(g, "HEAD:refs/heads/main");
const key = secret(`bf72-${Date.now()}-${process.pid}`);
const wantKey = createHash("sha256").update(key).digest("hex").slice(0, 16);
const host = new URL(EDGE).host;
const gh = spawnSync("gh", ["secret", "set", "DEPLOY_KEY", "-R", `${host}/${R}`], { input: key, encoding: "utf8",
  env: { ...process.env, GH_HOST: host, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" } });
log(`gh secret set DEPLOY_KEY -> ${gh.status} ${(gh.stderr || "").trim().slice(0, 200)}`);
if (gh.status !== 0) throw new Error("gh secret set");

// ---- runners: one ephemeral runner per slot at a time, while jobs are queued (as bf71.mjs)
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
async function startRunner(slot) {
  n += 1;
  const dir = join(out, `runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const k = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key: k, name: `bf72-${n}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf72-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slot.dir, f), { force: true });
  for (const d of ["_diag", "_work"]) rmSync(join(slot.dir, d), { recursive: true, force: true });
  slot.child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(join(out, `runner-${n}.out`))} 2>&1`], { cwd: slot.dir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  slot.child.stdin.end(config);
  log(`runner ${n} (id ${reg.runner})`);
}
const jobsOf = async (run) => {
  let r;
  for (let i = 0; (r = await api("GET", `${A}/runs/${run}`, admin)).status === 503 && i < 12; i++) await sleep(5000);
  return must("run", r).value.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
};
async function jobLog(id) {
  let text = "";
  for (let i = 0; i < 12; i++) {
    const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${id}/logs`, { headers: { authorization: `token ${admin}` } });
    text = await r.text();
    if (r.status === 200 && /Cleaning up orphan processes|Job completed|Post job cleanup/.test(text)) break;
    await sleep(5000);
  }
  writeFileSync(join(out, `job-${id}.log`), text);
  return text;
}

const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: "main" }), 201, 200).run;
log(`run ${run}`);
let jobs;
for (const t = Date.now(); ; ) {
  jobs = await jobsOf(run);
  row(join(out, "polls.jsonl"), { t: Date.now() - t, jobs: jobs.map((j) => [j.name, j.state]) });
  if (jobs.length && jobs.every((j) => j.state === "Completed")) break;
  if (Date.now() - t > 20 * 60_000) throw new Error("jobs still not done after 20 min");
  const queued = jobs.filter((j) => j.state === "Queued").length;
  for (const slot of slots) if (queued && (!slot.child || slot.child.exitCode !== null)) await startRunner(slot);
  await sleep(5000);
}
const per = [];
for (const j of jobs) {
  const text = await jobLog(j.id);
  const lines = text.split("\n").filter((l) => /BF72 |##\[error\]/.test(l) && !/echo |print\(/.test(l)).map((l) => l.replace(/^\S+\s/, "").slice(0, 300));
  per.push({ job: j.id, name: j.name, conclusion: j.s.Completed?.conclusion, lines });
}
// The run's number is its place among caller.yml's runs here, as BF-73's driver reads it (tests/m4/wp3/bf73/claims.mjs).
const adminLogin = must("user", await api("GET", "/api/v3/user", admin)).login;
const runs = (await api("GET", `${A}/runs`, admin)).body?.runs ?? [];
const runNumber = runs.filter((x) => x.workflow === WF).map((x) => Number(x.id)).filter((id) => id <= Number(run)).length;
const all = per.flatMap((j) => j.lines);
const has = (re) => all.some((l) => re.test(l));
const checks = {
  "every job succeeded": jobs.length === 3 && per.every((j) => j.conclusion === "Success"),
  "the called job ran as 'call / deploy'": per.some((j) => j.name === "call / deploy"),
  "typed inputs": has(/BF72 called target=prod-7 dry=true dry_is_bool=true/),
  "the inherited secret": has(new RegExp(`BF72 called key_sha=${wantKey}`)),
  "the tag, not the branch": has(/BF72 called version=v1/) && has(/url=https:\/\/prod-7\.example\/v1/),
  "the later job read the output": has(/BF72 after url=https:\/\/prod-7\.example\/v1 result=success/),
  "job_workflow_ref names the called file": has(new RegExp(`BF72 job_workflow_ref=m3org/${SHARED}/\\.github/workflows/deploy\\.yml@refs/tags/v1`)),
  "workflow_ref stays the caller's": has(new RegExp(`BF72 workflow_ref=${R}/\\.github/workflows/caller\\.yml@refs/heads/main`)),
  // BF-73's rows on a planned run: the dispatching admin and the run's number among caller.yml's runs here.
  "the called job's token carries the caller run's actor and number": has(new RegExp(`BF72 actor=${adminLogin} run_number=${runNumber}\\b`)),
};
const ok = Object.values(checks).every(Boolean);
const res = { ok, run, from: t0.toISOString(), to: new Date().toISOString(), checks, per };
row(join(out, "result.jsonl"), res);
for (const [k, v] of Object.entries(checks)) log(`${v ? "PASS" : "FAIL"} ${k}`);
log(`BF-72 ${ok ? "PASS" : "FAIL"}: run ${run}`);
log(`sheds: python3 tests/fanout/tieroff503.py ${res.from} ${res.to} graft-m4a-edge`);
const del = await api("DELETE", `/api/v3/repos/${R}/actions/secrets/DEPLOY_KEY`, admin);
log(`DEPLOY_KEY delete ${del.status}`);
for (const sl of slots) if (sl.child && sl.child.exitCode === null) try { process.kill(-sl.child.pid, "SIGTERM"); } catch {}
process.exit(ok ? 0 : 1);
