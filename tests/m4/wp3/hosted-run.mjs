// Runs an unmodified actions/runner against a hosted install's run service, through product routes only.
//   source tests/am1/lf13/env-m4a.sh   (GRAFT_EDGE, M3_STATE)
//   node tests/m4/wp3/hosted-run.mjs <runner dir> <workflow.yml> <out dir> [repo name] [kill-after-s]
// Creates `m3org/<repo name>` with the workflow at `.github/workflows/<file>` on `main` (once), registers an ephemeral
// runner for the repository (`POST /api/graftyard/v1/repos/{o}/{r}/actions/runners`), writes its JIT config with
// `examples/jitconfig.rs`, starts the runner, dispatches the workflow (`.../actions/dispatch`), and follows the run's
// jobs until each completed, starting a new runner after each job while jobs remain. With kill-after-s, the runner's
// process tree is killed that many seconds into the first job and the script then waits for the lease to end it
// (BF-4). Writes `<out>/run.jsonl` (one line per poll) and `<out>/runner-<n>.out`.
// `HOSTED_RUN_PR=1` starts the run as a same-repository pull request's instead of a dispatch (BF-70 r2: the site's
// `ActionsTrigger`, which sends the jobs' declared permissions where job tokens are on): `main` holds only a README,
// and each call pushes a fresh branch whose one commit adds the workflow, opens a pull request from it into `main`,
// and takes the `pull_request` run that appears for `refs/pull/N/merge`.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { Git, api, log, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const [runnerDir, wf, out, nameArg, killArg] = process.argv.slice(2);
const name = nameArg ?? `wp3-${Date.now().toString(36)}`;
const R = `m3org/${name}`;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const jit = join(repoRoot, "target/release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};

// The repository and its workflow, once.
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const path = `.github/workflows/${basename(wf)}`;
const asPr = process.env.HOSTED_RUN_PR === "1";
const branch = `bf-pr-${Date.now().toString(36)}`;
if (asPr) g.run(["checkout", "-q", "-b", branch]);
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, path), readFileSync(wf));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${path}`]).ok) {
  if (!g.run(["push", "-q", "origin", `HEAD:refs/heads/${asPr ? branch : "main"}`]).ok) throw new Error("push");
}
log(`repository ${R} has ${path} at ${g.run(["rev-parse", "HEAD"]).out.trim()}${asPr ? ` on ${branch}` : ""}`);

const A = `/api/graftyard/v1/repos/${R}/actions`;
let n = 0;
let child = null;
async function startRunner() {
  n += 1;
  const dir = join(out, `runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `${name}-${n}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `${name}-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(runnerDir, f), { force: true });
  rmSync(join(runnerDir, "_diag"), { recursive: true, force: true });
  const o = join(out, `runner-${n}.out`);
  writeFileSync(o, "");
  child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`], { cwd: runnerDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  child.stdin.end(config);
  log(`runner ${n} (id ${reg.runner}) started`);
  return child;
}

await startRunner();
// `HOSTED_RUN_FORK=1` marks the run as a fork pull request's (BF-59's fork-PR job: no fork route exists yet).
const fork = process.env.HOSTED_RUN_FORK === "1";
// A new repository's chain may answer 503 ("tierOff") for its first minutes (B0-02, repo:65596, about 4 min): retried
// every 2 s for up to 10 min, each answer recorded in `<out>/retries.jsonl`, and not counted against a row.
async function retry503(what, call) {
  const t0 = Date.now();
  for (;;) {
    const r = await call();
    if (r.status !== 503 || Date.now() - t0 > 600_000) return r;
    row(join(out, "retries.jsonl"), { what, t: Date.now() - t0, status: r.status, body: JSON.stringify(r.body).slice(0, 200) });
    await sleep(2000);
  }
}
let run;
if (asPr) {
  // The runs before this pull request's, so the new one is the run that was not listed.
  const before = new Set((must("runs", await retry503("runs", () => api("GET", `${A}/runs`, admin))).runs ?? []).map((r) => r.id));
  const gql = async (query, variables) => {
    const b = must("graphql", await retry503("graphql", () => api("POST", "/api/graphql", admin, { query, variables })));
    if (b.errors) throw new Error(`graphql: ${JSON.stringify(b.errors).slice(0, 400)}`);
    return b.data;
  };
  const repoId = (await gql(`query { repository(owner: "m3org", name: "${name}") { id } }`)).repository.id;
  const pr = (await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    { i: { repositoryId: repoId, baseRefName: "main", headRefName: branch, title: `${name} ${branch}`, body: "hosted-run.mjs" } })).createPullRequest.pullRequest;
  log(`pull request #${pr.number} from ${branch} into main`);
  const t = Date.now();
  for (;;) {
    const list = must("runs", await retry503("runs", () => api("GET", `${A}/runs`, admin))).runs ?? [];
    const r = list.find((r) => !before.has(r.id) && r.head_ref === `refs/pull/${pr.number}/merge`);
    if (r) { run = r.id; break; }
    if (Date.now() - t > 600_000) throw new Error(`no pull_request run for #${pr.number} in 10 min`);
    await sleep(3000);
  }
  log(`run ${run} created by pull request #${pr.number} after ${Math.round((Date.now() - t) / 1000)} s`);
} else {
  const d = must("dispatch", await retry503("dispatch", () => api("POST", `${A}/dispatch`, admin, { workflow: path, ref: "main", ...(fork ? { fork: true } : {}) })), 201, 200);
  run = d.run;
  log(`run ${run} created on ${d.shard} with ${d.jobs} jobs`);
}
const t0 = Date.now();
let killed = null;
for (;;) {
  const v = must("run", await retry503("run", () => api("GET", `${A}/runs/${run}`, admin))).value;
  const jobs = v.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
  row(join(out, "run.jsonl"), { t: Date.now() - t0, status: v.run.status, jobs });
  const running = jobs.find((j) => j.state === "InProgress");
  if (killArg && running && !killed) {
    await sleep(Number(killArg) * 1000);
    process.kill(-child.pid, "SIGKILL");
    killed = Date.now();
    log(`runner killed at ${new Date(killed).toISOString()} on job ${running.id}`);
  }
  if (jobs.every((j) => j.state === "Completed")) {
    log(`all ${jobs.length} jobs completed after ${Math.round((Date.now() - t0) / 1000)} s: ${JSON.stringify(jobs.map((j) => [j.id, j.s.Completed?.conclusion]))}`);
    break;
  }
  // An ephemeral runner exits after its job; another starts while jobs remain queued.
  if (!killed && child.exitCode !== null && jobs.some((j) => j.state === "Queued")) await startRunner();
  await sleep(5000);
}
