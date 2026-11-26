// F132's install half on graft-m4a (`tests/m4/wp3/f132/row.sh`; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/f132/push.mjs <out dir>          the repositories, the push, its run, and every check
//   node tests/m4/wp3/f132/push.mjs delete <out dir>   both repositories deleted at teardown
// Creates the public m3org/$F132_REPO-shared with `called.yml` (`on: workflow_call`) at tag v1, printing
// `version=tag`, and on a branch also named v1, printing `version=branch`. It pushes no workflow that runs on push.
// Then m3org/$F132_REPO gets `caller.yml` (`on: push`, one job that `uses:` the shared file at v1) pushed to main:
// that push is the check. Its run must be the one push run of `caller.yml` at the pushed commit, with the called job
// `call / deploy` succeeding on an unmodified runner (RUNNER). Its log names the tag's file, its ID token's
// `job_workflow_ref` is `m3org/<shared>/.github/workflows/called.yml@refs/tags/v1`, and its `workflow_ref` is the
// caller's file at `refs/heads/main` (GHES 3.22, "Using OpenID Connect with reusable workflows"). No token is printed.
// Writes <out>/result.json; exits 1 when a check fails.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const args = process.argv.slice(2);
const del = args[0] === "delete";
const out = del ? args[1] : args[0];
if (!out) throw new Error("usage: push.mjs [delete] <out dir>");
const name = process.env.F132_REPO ?? "f132";
const shared = `${name}-shared`;
const R = `m3org/${name}`, S = `m3org/${shared}`;
const admin = tokens.admin;
const A = `/api/graftyard/v1/repos/${R}/actions`;
const WF = ".github/workflows/caller.yml";
const SETTLE = Number(process.env.SETTLE_S ?? 60) * 1000;
const here = new URL(".", import.meta.url).pathname;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/;
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
  let ok = true;
  for (const x of [R, S]) {
    const d = await api("DELETE", `/api/v3/repos/${x}`, admin);
    const after = await api("GET", `/api/v3/repos/${x}`, null);
    log(`${x}: DELETE ${d.status}, anonymous read after ${after.status}`);
    ok &&= [204, 404].includes(d.status) && after.status === 404;
  }
  process.exit(ok ? 0 : 1);
}

const runnerDir = process.env.RUNNER;
if (!runnerDir) throw new Error("RUNNER: an unpacked actions/runner");
async function repo(n) {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: n, private: false, auto_init: true });
  if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must(`create ${n}`, made, 201);
  const g = new Git(`m3org/${n}`).as(admin);
  for (let i = 0; !g.clone().ok; i++) {
    if (i > 10) throw new Error(`clone ${n}`);
    await sleep(3000);
  }
  return g;
}
const gitOf = (g) => (...a) => {
  const r = g.run(a);
  if (!r.ok) throw new Error(`git ${a.join(" ")}: ${r.out}`);
  return r.out.trim();
};

// ---- the shared repository: called.yml at tag v1, and a branch v1 holding a different file
const s = await repo(shared);
const sg = gitOf(s);
const called = readFileSync(join(here, "called.yml"), "utf8");
mkdirSync(join(s.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(s.wt, ".github/workflows/called.yml"), called.replace("version=TAG", "version=tag"));
sg("add", "-A");
sg("commit", "-q", "-m", "F132 called at v1");
sg("tag", "v1");
const tagSha = sg("rev-parse", "HEAD");
sg("push", "-q", "origin", "HEAD:refs/heads/main", "refs/tags/v1:refs/tags/v1");
sg("checkout", "-q", "-b", "decoy");
writeFileSync(join(s.wt, ".github/workflows/called.yml"), called.replace("version=TAG", "version=branch"));
sg("commit", "-q", "-am", "F132 decoy branch v1");
const branchSha = sg("rev-parse", "HEAD");
sg("push", "-q", "origin", "HEAD:refs/heads/v1");
log(`${S}: tag v1 ${tagSha.slice(0, 12)}, branch v1 ${branchSha.slice(0, 12)}`);

// ---- the caller: caller.yml on main, whose push is the check
const g = await repo(name);
const cg = gitOf(g);
const runs = async () => must("runs", await retry503("runs", () => api("GET", `${A}/runs`, admin))).runs ?? [];
const before = new Set((await runs()).map((r) => r.id));
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, WF), readFileSync(join(here, "caller.yml"), "utf8").replace("SHARED/", `${S}/`));
cg("add", "-A");
cg("commit", "-q", "-m", "F132: a push that calls another repository's workflow");
const sha = cg("rev-parse", "HEAD");
cg("push", "-q", "origin", "HEAD:refs/heads/main");
const pushedAt = Date.now();
let got = [], last = Date.now();
for (;;) {
  for (const r of await runs()) if (!before.has(r.id) && !got.some((x) => x.id === r.id)) { got.push(r); last = Date.now(); }
  if (Date.now() - last > SETTLE && Date.now() - pushedAt > SETTLE) break;
  await sleep(5000);
}
log(`push ${sha.slice(0, 12)}: ${got.length} runs ${JSON.stringify(got.map((r) => [r.workflow, r.event, r.head_ref]))}`);

// ---- one ephemeral runner per queued job
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
    { cwd: runnerDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  child.stdin.end(config);
  const t0 = Date.now();
  while (child.exitCode === null && Date.now() - t0 < 15 * 60_000) await sleep(3000);
  if (child.exitCode === null) process.kill(-child.pid, "SIGKILL");
}
const mine = got.filter((r) => r.workflow === WF && r.event === "push" && r.head_sha === sha && r.head_ref === "refs/heads/main");
let jobs = [];
for (let i = 0; mine.length === 1 && i < 20; i++) {
  const v = must("run", await retry503("run", () => api("GET", `${A}/runs/${mine[0].id}`, admin))).value;
  jobs = v.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
  if (jobs.every((j) => j.state === "Completed")) break;
  if (jobs.some((j) => j.state === "Queued")) await runOne();
  else await sleep(5000);
}
const job = jobs.find((j) => j.name === "call / deploy");
let text = "";
for (let i = 0; i < 12 && job; i++) {
  const l = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, { headers: { authorization: `token ${admin}` } });
  text = await l.text();
  if (l.status === 200 && /F132 event_name=/.test(text)) break;
  await sleep(5000);
}
const said = text.split("\n").filter((l) => /F132 [a-z_]+=/.test(l) && !l.includes("echo ") && !l.includes("print("));
const line = (k) => said.map((l) => l.match(new RegExp(`F132 ${k}=(\\S+)`))?.[1]).find(Boolean) ?? null;
const want = {
  job_workflow_ref: `${S}/.github/workflows/called.yml@refs/tags/v1`,
  workflow_ref: `${R}/${WF}@refs/heads/main`,
};
const checks = {
  one_push_run: mine.length === 1 && got.length === 1,
  called_job_succeeded: jobs.length === 1 && job?.s.Completed?.conclusion === "Success",
  tag_not_branch: line("version") === "tag",
  job_workflow_ref: line("job_workflow_ref") === want.job_workflow_ref,
  workflow_ref: line("workflow_ref") === want.workflow_ref,
  event_name: line("event_name") === "push",
  no_token_in_logs: !jwt.test(text),
};
const ok = Object.values(checks).every((v) => v === true);
writeFileSync(join(out, "result.json"), JSON.stringify({ caller: R, shared: S, sha, tagSha, branchSha,
  runs: got.map((r) => [r.id, r.workflow, r.event, r.head_ref, r.head_sha]), jobs: jobs.map((j) => [j.id, j.name, j.s.Completed?.conclusion ?? j.state]),
  read: { version: line("version"), job_workflow_ref: line("job_workflow_ref"), workflow_ref: line("workflow_ref"), event_name: line("event_name") },
  want, checks, ok, runners: nRunner }, null, 2));
log(`F132: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
if (!ok) process.exit(1);
