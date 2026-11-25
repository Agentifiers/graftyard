// BF-78's install half on graft-m4a (`tests/m4/wp3/bf78/row.sh`; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/bf78/push.mjs <out dir>          the pushes, their runs, and every check
//   node tests/m4/wp3/bf78/push.mjs delete <out dir>   the repository deleted at teardown
// Creates m3org/$BF78_REPO with the five workflows of this directory on `main` (that push's own runs are recorded and
// waited out first), then makes the row's pushes in order with `git push` as the admin:
//   p1 main, changing src/       p2 main, docs only           p3 new branch rel/1.0
//   p4 new branch rel/1.0-wip    p5 new branch dev            p6 tag v1 (lightweight, at main)
//   p7 force push of dev         p8 delete of dev             p9 main, "[skip ci]" in its message
// For each, it waits until no new run appears for SETTLE_S (default 60 s) and records the runs whose `head_ref` is the
// pushed ref and whose `head_sha` is the pushed SHA. The expected set is written out below from the filters by hand,
// not computed from the install's filter code. Every queued job runs on an unmodified runner (RUNNER) registered for
// the repository, one ephemeral runner per job; each `any.yml` job writes `github.event` and an ID token for audience
// bf78 to <out>/jobs on this host. Then, for each `any.yml` run: the job message's context line, `github.event` (ref,
// before, after, created, deleted, forced, head_commit.id and message, pusher, sender), and the token through
// `tests/m4/wp3/bf74/rp.py` with `sub` `repo:m3org/<repo>:ref:<ref>` and `actor` the admin's login. Writes
// <out>/result.json; exits 1 when a check fails. No token is printed.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const args = process.argv.slice(2);
const del = args[0] === "delete";
const out = del ? args[1] : args[0];
if (!out) throw new Error("usage: push.mjs [delete] <out dir>");
const name = process.env.BF78_REPO ?? "bf78";
const R = `m3org/${name}`;
const admin = tokens.admin;
const A = `/api/graftyard/v1/repos/${R}/actions`;
const SETTLE = Number(process.env.SETTLE_S ?? 60) * 1000;
const here = new URL(".", import.meta.url).pathname;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/;
mkdirSync(out, { recursive: true });
const result = (v) => writeFileSync(join(out, "result.json"), JSON.stringify(v, null, 2));
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
const me = must("user", await api("GET", "/api/v3/user", admin));
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
    { cwd: runnerDir, stdio: ["pipe", "ignore", "ignore"], detached: true, env: { ...process.env, BF78_OUT: jobsDir } });
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
      states.push(...v.jobs.map((j) => ({ run: id, job: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state })));
    }
    if (states.every((j) => j.state === "Completed")) return states;
    if (states.some((j) => j.state === "Queued")) await runOne();
    else await sleep(5000);
  }
}

// ---- setup: the workflows on main
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
for (const f of ["any.yml", "branches.yml", "ignore.yml", "tags.yml", "paths.yml"]) {
  writeFileSync(join(g.wt, ".github/workflows", f), readFileSync(join(here, f)));
}
mkdirSync(join(g.wt, "src"), { recursive: true });
writeFileSync(join(g.wt, "src/a.txt"), "a\n");
git("add", "-A");
git("commit", "-q", "-m", "Add BF-78's workflows");
git("push", "-q", "origin", "HEAD:refs/heads/main");
const setup = await settle();
log(`setup push: ${setup.length} runs ${JSON.stringify(setup.map((r) => r.workflow))}`);
await drain(setup.map((r) => r.id));

// ---- the pushes, each with its expected workflows (from the filters, by hand)
const W = (f) => `.github/workflows/${f}`;
const commit = (path, text, msg) => {
  mkdirSync(join(g.wt, path, ".."), { recursive: true });
  writeFileSync(join(g.wt, path), text);
  git("add", "-A");
  git("commit", "-q", "-m", msg);
  return git("rev-parse", "HEAD");
};
const pushes = [];
async function push(id, ref, make, want, extra = []) {
  const sha = make();
  const before = g.remoteHead(ref) || "0".repeat(40);
  const at = Date.now();
  const r = g.run(["push", "-q", ...extra, "origin", sha ? `${sha}:${ref}` : `:${ref}`]);
  if (!r.ok) throw new Error(`push ${id}: ${r.out}`);
  const got = await settle();
  pushes.push({ id, ref, sha, before, at, want: want.sort(), got });
  log(`${id} ${ref} ${sha ? sha.slice(0, 12) : "(delete)"}: ${got.length} runs ${JSON.stringify(got.map((x) => x.workflow))}`);
}
git("checkout", "-q", "main");
await push("p1", "refs/heads/main", () => commit("src/b.txt", "b\n", "p1: change src"), [W("any.yml"), W("branches.yml"), W("ignore.yml"), W("paths.yml")]);
await push("p2", "refs/heads/main", () => commit("docs/c.md", "c\n", "p2: docs only"), [W("any.yml"), W("branches.yml"), W("ignore.yml")]);
git("checkout", "-q", "-b", "rel/1.0");
await push("p3", "refs/heads/rel/1.0", () => commit("rel.txt", "r\n", "p3: rel/1.0"), [W("any.yml"), W("branches.yml")]);
git("checkout", "-q", "-b", "rel/1.0-wip");
await push("p4", "refs/heads/rel/1.0-wip", () => commit("wip.txt", "w\n", "p4: rel/1.0-wip"), [W("any.yml")]);
git("checkout", "-q", "main");
git("checkout", "-q", "-b", "dev");
await push("p5", "refs/heads/dev", () => commit("dev.txt", "1\n", "p5: dev"), [W("any.yml"), W("ignore.yml")]);
git("checkout", "-q", "main");
await push("p6", "refs/tags/v1", () => { git("tag", "v1"); return git("rev-parse", "v1^{commit}"); }, [W("any.yml"), W("tags.yml")]);
git("checkout", "-q", "dev");
git("reset", "-q", "--hard", "main");
await push("p7", "refs/heads/dev", () => commit("dev.txt", "2\n", "p7: dev rewritten"), [W("any.yml"), W("ignore.yml")], ["--force"]);
await push("p8", "refs/heads/dev", () => "", []);
git("checkout", "-q", "main");
await push("p9", "refs/heads/main", () => commit("src/d.txt", "d\n", "p9: skipped [skip ci]"), []);

// ---- run every job, then read what each `any.yml` job wrote
const all = pushes.flatMap((p) => p.got.map((r) => r.id));
const states = await drain(all);
const checks = {};
const per = [];
for (const p of pushes) {
  const mine = p.got.filter((r) => r.head_ref === p.ref && r.head_sha === p.sha && r.event === "push");
  const wf = mine.map((r) => r.workflow).sort();
  const c = {
    exact: JSON.stringify(wf) === JSON.stringify(p.want),
    none_elsewhere: mine.length === p.got.length,
    one_each: new Set(wf).size === wf.length,
  };
  const anyRun = mine.find((r) => r.workflow === W("any.yml"));
  if (anyRun) {
    const ev = existsSync(join(jobsDir, `event-${anyRun.id}.json`)) ? JSON.parse(readFileSync(join(jobsDir, `event-${anyRun.id}.json`), "utf8")) : null;
    const created = p.before === "0".repeat(40);
    c.event = !!ev && ev.ref === p.ref && ev.after === p.sha && ev.before === p.before && ev.created === created &&
      ev.deleted === false && ev.forced === (p.id === "p7") && ev.head_commit?.id === p.sha && ev.pusher?.name === me.login &&
      ev.sender?.login === me.login && ev.repository?.full_name === R && Array.isArray(ev.commits) &&
      !("added" in (ev.head_commit ?? {}));
    const tok = join(jobsDir, `token-${anyRun.id}.jwt`);
    const want = { sub: `repo:${R}:ref:${p.ref}`, actor: me.login, actor_id: String(me.id), ref: p.ref, event_name: "push" };
    const x = spawnSync("python3", [join(repoRoot, "tests/m4/wp3/bf74/rp.py"), tok, "--aud", "bf78",
      ...(new URL(EDGE).hostname.endsWith(".workers.dev") ? ["--agent", "bf78-rp"] : []),
      ...Object.entries(want).flatMap(([k, v]) => ["--want", `${k}=${v}`])], { encoding: "utf8" });
    const rp = existsSync(tok) ? JSON.parse(x.stdout.trim().split("\n").pop() || "{}") : {};
    c.token = x.status === 0 && rp.accepted === true;
    const job = states.find((j) => j.run === anyRun.id);
    c.job_succeeded = job?.s.Completed?.conclusion === "Success";
    let text = "";
    for (let i = 0; i < 12 && job; i++) {
      const l = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.job}/logs`, { headers: { authorization: `token ${admin}` } });
      text = await l.text();
      if (l.status === 200 && /Complete job|Cleaning up orphan/.test(text)) break;
      await sleep(5000);
    }
    const said = text.split("\n").filter((l) => !l.includes("##[group]") && !l.includes("\u001b[36;1m")).join("\n");
    c.context = said.includes(`context event=push ref=${p.ref} sha=${p.sha} actor=${me.login}`);
    c.no_token_in_logs = !jwt.test(text);
    rmSync(tok, { force: true });
    per.push({ id: p.id, run: anyRun.id, event: ev && { ref: ev.ref, before: ev.before, after: ev.after, created: ev.created, forced: ev.forced, commits: ev.commits?.length, head: ev.head_commit?.message }, claims: rp.claims, mismatch: rp.mismatch });
  }
  checks[p.id] = c;
}
const ok = Object.values(checks).every((c) => Object.values(c).every((v) => v === true));
result({ repo: R, actor: me.login, pushes: pushes.map(({ got, ...p }) => ({ ...p, got: got.map((r) => [r.id, r.workflow, r.head_ref, r.head_sha]) })), per, checks, ok, runners: nRunner });
log(`BF-78: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
if (!ok) process.exit(1);
