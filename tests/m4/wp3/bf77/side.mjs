// Two side checks on BF-77's rerun day on graft-m4a (the lead's GO, 2026-11-26), each at most a run:
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp3/bf77/side.mjs f132 <out dir>     push-calls' edge half
//   node tests/m4/wp3/bf77/side.mjs token <token file>  F129: one job's ID token, written to the orb (never printed)
//   node tests/m4/wp3/bf77/side.mjs delete             every repository this file made, deleted
// f132: m3org/bf77-shared (public) holds F132's `called.yml` at tag v1 and a decoy branch v1, as
// `tests/m4/wp3/f132/push.mjs` makes them; m3org/bf77-caller holds `caller.yml` with `on: workflow_dispatch` in place
// of `on: push`, and one dispatch starts it. Pass: the called job `call / deploy` succeeds, prints the tag's
// `version=tag`, and its ID token's `job_workflow_ref` is `m3org/bf77-shared/.github/workflows/called.yml@refs/tags/v1`.
// token: `tests/m4/wp3/bf74/token.mjs` in m3org/bf77-f129 (audience bf74), for `lag.py`.
// RUNNER is an unpacked actions/runner 2.338.2; target/release/examples/jitconfig must be built.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { EDGE, Git, api, log, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [what, arg] = process.argv.slice(2);
const admin = tokens.admin;
const here = new URL(".", import.meta.url).pathname;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const S = "m3org/bf77-shared", R = "m3org/bf77-caller", T = "m3org/bf77-f129";
const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/;
const must = (w, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body;
};
async function retry503(call) {
  for (const t0 = Date.now(); ;) {
    const r = await call();
    if (r.status !== 503 || Date.now() - t0 > 300_000) return r;
    log(`503 ${JSON.stringify(r.body).slice(0, 160)}`);
    await sleep(2000);
  }
}

if (what === "delete") {
  let ok = true;
  for (const x of [R, S, T]) {
    const d = await api("DELETE", `/api/v3/repos/${x}`, admin);
    let after = await api("GET", `/api/v3/repos/${x}`, null);
    for (const t0 = Date.now(); after.status !== 404 && Date.now() - t0 < 120_000;) { await sleep(5000); after = await api("GET", `/api/v3/repos/${x}`, null); }
    log(`${x}: DELETE ${d.status}, anonymous read ${after.status}`);
    ok &&= [204, 404].includes(d.status) && after.status === 404;
  }
  process.exit(ok ? 0 : 1);
}

if (what === "token") {
  const r = spawnSync("node", [join(here, "../bf74/token.mjs"), arg], { env: { ...process.env, BF74_REPO: "bf77-f129" }, stdio: "inherit", timeout: 20 * 60_000 });
  process.exit(r.status ?? 1);
}

if (what !== "f132") throw new Error("usage: side.mjs f132 <out> | token <file> | delete");
const out = resolve(arg);
mkdirSync(out, { recursive: true });
async function repo(full, priv) {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: full.split("/")[1], private: priv, auto_init: true });
  if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must(`create ${full}`, made, 201);
  const g = new Git(full).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error(`clone ${full}`); await sleep(3000); }
  return (...a) => { const r = g.run(a); if (!r.ok) throw new Error(`git ${a.join(" ")}: ${r.out}`); return r.out.trim(); };
}
const called = readFileSync(join(here, "../f132/called.yml"), "utf8");
const sg = await repo(S, false);
const sw = sg("rev-parse", "--show-toplevel");
mkdirSync(join(sw, ".github/workflows"), { recursive: true });
writeFileSync(join(sw, ".github/workflows/called.yml"), called.replace("version=TAG", "version=tag"));
// A second call finds both repositories made and reuses them (and RUN, a run already dispatched, when given).
const fresh = !sg("ls-remote", "origin", "refs/tags/v1");
if (fresh) {
  sg("add", "-A"); sg("commit", "-q", "-m", "F132 called at v1"); sg("tag", "v1");
  sg("push", "-q", "origin", "HEAD:refs/heads/main", "refs/tags/v1:refs/tags/v1");
  sg("checkout", "-q", "-b", "decoy");
  writeFileSync(join(sw, ".github/workflows/called.yml"), called.replace("version=TAG", "version=branch"));
  sg("commit", "-q", "-am", "F132 decoy branch v1");
  sg("push", "-q", "origin", "HEAD:refs/heads/v1");
}
const tagSha = sg("ls-remote", "origin", "refs/tags/v1").split(/\s/)[0];
log(`${S}: tag v1 ${tagSha.slice(0, 12)}`);

const cg = await repo(R, false);
const cw = cg("rev-parse", "--show-toplevel");
const WF = ".github/workflows/caller.yml";
mkdirSync(join(cw, ".github/workflows"), { recursive: true });
writeFileSync(join(cw, WF), readFileSync(join(here, "../f132/caller.yml"), "utf8").replace("on: push", "on: workflow_dispatch").replace("SHARED/", `${S}/`));
cg("add", "-A");
if (cg("status", "--porcelain")) {
  cg("commit", "-q", "-m", "F132 edge half: a dispatch that calls another repository's workflow");
  cg("push", "-q", "origin", "HEAD:refs/heads/main");
}
const A = `/api/graftyard/v1/repos/${R}/actions`;
const d = process.env.RUN ? { run: Number(process.env.RUN), shard: "(reused)", jobs: "?" }
  : must("dispatch", await retry503(() => api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: "main" })), 201, 200);
log(`dispatch: run ${d.run} on ${d.shard}, ${d.jobs} jobs`);
const runnerDir = resolve(process.env.RUNNER);
const jit = join(repoRoot, "target/release/examples/jitconfig");
let n = 0, child = null, jobs = [];
for (const t0 = Date.now(); Date.now() - t0 < 15 * 60_000;) {
  const v = must("run", await retry503(() => api("GET", `${A}/runs/${d.run}`, admin))).value;
  jobs = v.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
  if (jobs.length && jobs.every((j) => j.state === "Completed")) break;
  if (jobs.some((j) => j.state === "Queued") && (!child || child.exitCode !== null)) {
    n += 1;
    const dir = join(out, `runner-${n}`);
    mkdirSync(dir, { recursive: true });
    const key = execFileSync(jit, ["keygen", dir]).toString().trim();
    const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf77-f132-${n}` }), 201);
    writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
    const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf77-f132-${n}`]).toString();
    for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(runnerDir, f), { force: true });
    for (const x of ["_diag", "_work"]) rmSync(join(runnerDir, x), { recursive: true, force: true });
    child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(join(out, `runner-${n}.out`))} 2>&1`], { cwd: runnerDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
    child.stdin.end(config);
  }
  await sleep(5000);
}
if (child && child.exitCode === null) try { process.kill(-child.pid, "SIGTERM"); } catch {}
const job = jobs.find((j) => j.name === "call / deploy");
let text = "";
for (let i = 0; i < 12 && job; i++) {
  const l = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, { headers: { authorization: `token ${admin}` } });
  text = await l.text();
  if (l.status === 200 && /F132 event_name=/.test(text)) break;
  await sleep(5000);
}
writeFileSync(join(out, `job-${job?.id}.log`), text.replace(jwt, "<jwt>"));
const said = text.split("\n").filter((l) => /F132 [a-z_]+=/.test(l) && !l.includes("echo ") && !l.includes("print("));
const line = (k) => said.map((l) => l.match(new RegExp(`F132 ${k}=(\\S+)`))?.[1]).find(Boolean) ?? null;
const checks = {
  called_job_succeeded: jobs.length === 1 && job?.s.Completed?.conclusion === "Success",
  tag_not_branch: line("version") === "tag",
  job_workflow_ref: line("job_workflow_ref") === `${S}/.github/workflows/called.yml@refs/tags/v1`,
  workflow_ref: line("workflow_ref") === `${R}/${WF}@refs/heads/main`,
  event_name: line("event_name") === "workflow_dispatch",
  no_token_in_logs: !jwt.test(text),
};
const ok = Object.values(checks).every(Boolean);
const res = { run: d.run, jobs: jobs.map((j) => [j.id, j.name, j.s.Completed?.conclusion ?? j.state]), read: { version: line("version"), job_workflow_ref: line("job_workflow_ref"), workflow_ref: line("workflow_ref"), event_name: line("event_name") }, checks, ok };
writeFileSync(join(out, "result.json"), JSON.stringify(res, null, 2));
log(`F132 dispatch: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(res)}`);
process.exit(ok ? 0 : 1);
