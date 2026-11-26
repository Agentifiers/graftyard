// F133's install half on graft-m4a (`tests/m4/wp3/f133/row.sh`; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/f133/push.mjs <out dir>          the pushes, their runs, and every check
//   node tests/m4/wp3/f133/push.mjs delete <out dir>   the repository deleted at teardown
// Creates m3org/$F133_REPO with `any.yml` and `paths.yml` (`paths: ['src/**']`) on `main` (that push's runs are
// waited out first), then pushes as the admin, in order:
//   q1 new branch feat off main, two commits under docs/   q2 new branch feat-src off main, one commit under src/
//   q3 feat fast-forward, adding src/x.txt                q4 force push of feat: main plus src/x.txt as q3 left it
//                                                           and a docs/ change, so before..after changes docs/ only
//   q5 force push of feat: main plus a docs/ change, so before..after removes src/x.txt
// The expected workflows are written out below from GHES's rules by hand ("Workflow syntax", "Git diff comparisons";
// the operator's ruling for a new branch, its merge base with the default branch): q1 and q4 start no paths run, which
// they did before F133's fix, and q4's three-dot diff from the merge base would add src/x.txt. Each `any.yml` job writes
// `github.event` to <out>/jobs; the checks read `commits`' ids against the pushed commits, oldest first: q1 lists both
// of its commits (before the fix, its head alone). Every queued job runs on an unmodified runner (RUNNER), one
// ephemeral runner per job. Writes <out>/result.json; exits 1 when a check fails.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const args = process.argv.slice(2);
const del = args[0] === "delete";
const out = del ? args[1] : args[0];
if (!out) throw new Error("usage: push.mjs [delete] <out dir>");
const name = process.env.F133_REPO ?? "f133";
const R = `m3org/${name}`;
const admin = tokens.admin;
const A = `/api/graftyard/v1/repos/${R}/actions`;
const SETTLE = Number(process.env.SETTLE_S ?? 60) * 1000;
const here = new URL(".", import.meta.url).pathname;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
mkdirSync(out, { recursive: true });
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body;
};
// A new repository's chain may answer 503 tierOff for its first minutes (B0-02): retried every 2 s, recorded.
async function retry503(what, call) {
  const t0 = Date.now();
  for (;;) {
    const r = await call();
    if (r.status !== 503 || Date.now() - t0 > 600_000) return r;
    row(join(out, "retries.jsonl"), { what, t: Date.now() - t0, body: JSON.stringify(r.body).slice(0, 200) });
    await sleep(2000);
  }
}

if (del) {
  const d = await api("DELETE", `/api/v3/repos/${R}`, admin);
  const after = await api("GET", `/api/v3/repos/${R}`, null);
  log(`${R}: DELETE ${d.status}, anonymous read after ${after.status}`);
  process.exit([204, 404].includes(d.status) && after.status === 404 ? 0 : 1);
}

const runnerDir = process.env.RUNNER;
if (!runnerDir) throw new Error("RUNNER: an unpacked actions/runner");
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const git = (...a) => {
  const r = g.run(a);
  if (!r.ok) throw new Error(`git ${a.join(" ")}: ${r.out}`);
  return r.out.trim();
};
const runs = async () => must("runs", await retry503("runs", () => api("GET", `${A}/runs`, admin))).runs ?? [];
const seen = new Set();
// The runs that appeared since the last call, once no new one has for SETTLE.
async function settle() {
  let last = Date.now();
  const got = [];
  for (;;) {
    for (const r of await runs()) if (!seen.has(r.id)) { seen.add(r.id); got.push(r); last = Date.now(); }
    if (Date.now() - last > SETTLE) return got;
    await sleep(5000);
  }
}

// ---- jobs: one ephemeral runner per queued job, until none is queued or running
const jobsDir = join(out, "jobs");
mkdirSync(jobsDir, { recursive: true, mode: 0o700 });
const jit = join(repoRoot, "target/release/examples/jitconfig");
let nRunner = 0;
async function runOne() {
  nRunner += 1;
  const dir = join(out, `runner-${nRunner}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `${name}-${nRunner}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `${name}-${nRunner}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(runnerDir, f), { force: true });
  rmSync(join(runnerDir, "_work"), { recursive: true, force: true });
  const o = join(out, `runner-${nRunner}.out`);
  const child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`],
    { cwd: runnerDir, stdio: ["pipe", "ignore", "ignore"], detached: true, env: { ...process.env, F133_OUT: jobsDir } });
  child.stdin.end(config);
  const t0 = Date.now();
  while (child.exitCode === null && Date.now() - t0 < 15 * 60_000) await sleep(3000);
  if (child.exitCode === null) process.kill(-child.pid, "SIGKILL");
}
async function drain(ids) {
  for (;;) {
    const states = [];
    for (const id of ids) {
      const v = must("run", await retry503("run", () => api("GET", `${A}/runs/${id}`, admin))).value;
      states.push(...v.jobs.map((j) => ({ run: id, job: j.spec.id, state: Object.keys(j.state)[0], s: j.state })));
    }
    if (states.every((j) => j.state === "Completed")) return states;
    if (states.some((j) => j.state === "Queued")) await runOne();
    else await sleep(5000);
  }
}

// ---- setup: the workflows and src/a.txt on main
const commit = (path, text, msg) => {
  mkdirSync(join(g.wt, path, ".."), { recursive: true });
  writeFileSync(join(g.wt, path), text);
  git("add", "-A");
  git("commit", "-q", "-m", msg);
  return git("rev-parse", "HEAD");
};
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
for (const f of ["any.yml", "paths.yml"]) writeFileSync(join(g.wt, ".github/workflows", f), readFileSync(join(here, f)));
commit("src/a.txt", "a\n", "Add F133's workflows");
git("push", "-q", "origin", "HEAD:refs/heads/main");
const setup = await settle();
log(`setup push: ${setup.length} runs ${JSON.stringify(setup.map((r) => r.workflow))}`);
await drain(setup.map((r) => r.id));
const main = git("rev-parse", "HEAD");

// ---- the pushes, each with its expected workflows and pushed commits (by hand)
const W = (f) => `.github/workflows/${f}`;
const pushes = [];
async function push(id, ref, shas, want, extra = []) {
  const sha = shas.at(-1);
  const before = g.remoteHead(ref) || "0".repeat(40);
  const r = g.run(["push", "-q", ...extra, "origin", `${sha}:${ref}`]);
  if (!r.ok) throw new Error(`push ${id}: ${r.out}`);
  const got = await settle();
  pushes.push({ id, ref, sha, before, commits: shas, want: want.sort(), got });
  log(`${id} ${ref} ${sha.slice(0, 12)}: ${got.length} runs ${JSON.stringify(got.map((x) => x.workflow))}`);
}
git("checkout", "-q", "-b", "feat", main);
const f1 = commit("docs/one.md", "1\n", "q1: docs one");
const f2 = commit("docs/two.md", "2\n", "q1: docs two");
await push("q1", "refs/heads/feat", [f1, f2], [W("any.yml")]);
git("checkout", "-q", "-b", "feat-src", main);
await push("q2", "refs/heads/feat-src", [commit("src/s.txt", "s\n", "q2: src")], [W("any.yml"), W("paths.yml")]);
git("checkout", "-q", "feat");
await push("q3", "refs/heads/feat", [commit("src/x.txt", "x\n", "q3: src x")], [W("any.yml"), W("paths.yml")]);
git("reset", "-q", "--hard", main);
mkdirSync(join(g.wt, "src"), { recursive: true });
writeFileSync(join(g.wt, "src/x.txt"), "x\n");
await push("q4", "refs/heads/feat", [commit("docs/three.md", "3\n", "q4: src x as before, docs three")], [W("any.yml")], ["--force"]);
git("reset", "-q", "--hard", main);
await push("q5", "refs/heads/feat", [commit("docs/four.md", "4\n", "q5: docs four, src x gone")], [W("any.yml"), W("paths.yml")], ["--force"]);

// ---- run every job, then read what each `any.yml` job wrote
const states = await drain(pushes.flatMap((p) => p.got.map((r) => r.id)));
const checks = {};
const per = [];
for (const p of pushes) {
  const mine = p.got.filter((r) => r.head_ref === p.ref && r.head_sha === p.sha && r.event === "push");
  const wf = mine.map((r) => r.workflow).sort();
  const c = { exact: JSON.stringify(wf) === JSON.stringify(p.want), none_elsewhere: mine.length === p.got.length };
  const anyRun = mine.find((r) => r.workflow === W("any.yml"));
  const f = anyRun && join(jobsDir, `event-${anyRun.id}.json`);
  const ev = f && existsSync(f) ? JSON.parse(readFileSync(f, "utf8")) : null;
  const ids = ev?.commits?.map((x) => x.id) ?? null;
  c.commits = JSON.stringify(ids) === JSON.stringify(p.commits);
  c.forced = ev?.forced === ["q4", "q5"].includes(p.id);
  c.jobs_succeeded = states.filter((j) => mine.some((r) => r.id === j.run)).every((j) => j.s.Completed?.conclusion === "Success");
  per.push({ id: p.id, ref: p.ref, before: p.before, after: p.sha, runs: wf, commits: ids, forced: ev?.forced, created: ev?.created });
  checks[p.id] = c;
}
const ok = Object.values(checks).every((c) => Object.values(c).every((v) => v === true));
writeFileSync(join(out, "result.json"), JSON.stringify({ repo: R, main, pushes: pushes.map(({ got, ...p }) => ({ ...p, got: got.map((r) => [r.id, r.workflow, r.head_ref, r.head_sha]) })), per, checks, ok, runners: nRunner }, null, 2));
log(`F133: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
if (!ok) process.exit(1);
