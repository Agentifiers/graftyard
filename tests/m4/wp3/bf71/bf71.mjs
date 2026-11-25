// BF-71 on graft-m4a (build/BUILD-PLAN.md, M4-WP3; decisions/DECISIONS.md, "`actions-artifacts`"): an unmodified
// actions/runner 2.338.2 runs tests/m4/wp3/bf71/art.yml with upload-artifact@v3.2.2 and download-artifact@v3.1.0
// mirrored unchanged under `actions-mirror` (m4a holds `actions/*` Deleted after BF-69, so the mirror is
// tests/m4/wp3/bf69/mirror.mjs with MIRROR_ORG=actions-mirror; the workflow's `uses:` are rewritten to that name).
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf71/bf71.mjs lifecycle            # the actions/artifacts/ lifecycle rule, merged and read back
//   node tests/m4/wp3/bf71/bf71.mjs run <out> <runner dir>...
//   node tests/m4/wp3/bf71/bf71.mjs prefix <out> <chain>/<repo>   # R2 objects left under that repository's prefix
// `run` writes one JSON line into <out>/result.jsonl: every job's conclusion, the `BF71 ALL SUMS OK` and `BF71 PROBE`
// lines, the R2 objects the run left (records, manifests, chunks), and the window for tests/fanout/tieroff503.py.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [what, out, ...rest] = process.argv.slice(2);
const CF = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" };
const BUCKETS = ["a", "b", "c"].map((s) => `graft-m4a-site-${s}-packs`);
const RULE = { id: "graftyard-actions-artifacts-90d", enabled: true, conditions: { prefix: "actions/artifacts/" },
  deleteObjectsTransition: { condition: { type: "Age", maxAge: 90 * 86400 } } };
const cf = async (method, path, body) => {
  const r = await fetch(`${CF}${path}`, { method, headers: H, body: body === undefined ? undefined : JSON.stringify(body) });
  const j = await r.json();
  if (!j.success) throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(j.errors).slice(0, 300)}`);
  return j.result;
};
async function objects(bucket, prefix) {
  const all = [];
  for (let cursor = ""; ; ) {
    const r = await fetch(`${CF}/r2/buckets/${bucket}/objects?prefix=${encodeURIComponent(prefix)}&per_page=1000${cursor ? `&cursor=${cursor}` : ""}`, { headers: H });
    const j = await r.json();
    all.push(...(j.result ?? []).map((o) => ({ key: o.key, size: o.size })));
    cursor = j.result_info?.cursor;
    if (!j.result_info?.is_truncated || !cursor) return all;
  }
}

if (what === "lifecycle") {
  // Install configuration, never a request path. A bucket's rules are replaced by a PUT, so the PUT carries every rule
  // it held, with ours added or updated in place, and the read after must hold all of them.
  for (const b of BUCKETS) {
    const before = (await cf("GET", `/r2/buckets/${b}/lifecycle`)).rules ?? [];
    const rules = [...before.filter((r) => r.id !== RULE.id), RULE];
    await cf("PUT", `/r2/buckets/${b}/lifecycle`, { rules });
    const after = (await cf("GET", `/r2/buckets/${b}/lifecycle`)).rules ?? [];
    const ours = after.find((r) => r.id === RULE.id);
    const kept = before.filter((r) => r.id !== RULE.id).every((r) => after.some((x) => x.id === r.id));
    const ok = kept && ours?.conditions?.prefix === "actions/artifacts/" && ours?.deleteObjectsTransition?.condition?.maxAge === 90 * 86400;
    log(`${b}: ${before.length} rule(s) before, ${after.length} after, ours ${JSON.stringify(ours)}, others kept ${kept}: ${ok ? "OK" : "FAIL"}`);
    if (!ok) process.exit(1);
  }
  process.exit(0);
}

if (what === "prefix") {
  for (const b of BUCKETS) log(`${b} actions/artifacts/${rest[0]}/: ${(await objects(b, `actions/artifacts/${rest[0]}/`)).length} objects`);
  process.exit(0);
}

if (what !== "run") throw new Error("usage: bf71.mjs lifecycle | run <out> <runner dir>... | prefix <chain>/<repo>");
const runnerDirs = rest;
const NAME = process.env.BF71_REPO ?? "bf71";
const R = `m3org/${NAME}`, A = `/api/graftyard/v1/repos/${R}/actions`, WF = ".github/workflows/art.yml";
const admin = tokens.admin;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const must = (w, r, ...ok) => { if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`); return r.body; };
const t0 = new Date();

// ---- the repository and the workflow, `uses:` pointed at the mirror
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: false, auto_init: true });
if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
const mirror = process.env.MIRROR_ORG ?? "actions-mirror";
writeFileSync(join(g.wt, WF), readFileSync(new URL("./art.yml", import.meta.url), "utf8").replaceAll("uses: actions/", `uses: ${mirror}/`));
g.run(["add", "-A"]);
g.run(["commit", "-q", "--allow-empty", "-m", "BF-71"]);
if (!g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push");

// ---- runners: one ephemeral runner per slot at a time, while jobs are queued (as bf69.mjs)
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
async function startRunner(slot) {
  n += 1;
  const dir = join(out, `runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf71-${n}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf71-${n}`]).toString();
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
  if (Date.now() - t > 30 * 60_000) throw new Error("jobs still not done after 30 min");
  const queued = jobs.filter((j) => j.state === "Queued" || j.state === "Pending").length;
  for (const slot of slots) if (queued && (!slot.child || slot.child.exitCode !== null)) await startRunner(slot);
  await sleep(5000);
}
const per = [];
for (const j of jobs) {
  const text = await jobLog(j.id);
  const lines = text.split("\n").filter((l) => /BF71 |sha256sum: WARNING|: FAILED|##\[error\]/.test(l) && !/echo |console\.log/.test(l)).map((l) => l.replace(/^\S+\s/, "").slice(0, 400));
  per.push({ job: j.id, name: j.name, conclusion: j.s.Completed?.conclusion, lines });
}
// What the run left in R2: the run's prefix (`actions/artifacts/<shard>/<repo>/<run>/`) in the owner's home bucket.
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const left = {};
for (const b of BUCKETS) {
  const o = (await objects(b, "actions/artifacts/")).filter((x) => x.key.includes(`/${repoId}/${run}/`));
  if (o.length) left[b] = { prefix: o[0].key.split(`/${run}/`)[0] + `/${run}/`, objects: o.length, bytes: o.reduce((t, x) => t + x.size, 0),
    records: o.filter((x) => x.key.includes(`/${run}/i/`)).length, manifests: o.filter((x) => x.key.includes(`/${run}/m/`)).map((x) => x.key.split(`/${run}/m/`)[1]) };
}
const all = (re) => per.some((j) => j.lines.some((l) => re.test(l)));
const ok = jobs.length === 6 && per.every((j) => j.conclusion === "Success") && all(/BF71 ALL SUMS OK/) && all(/BF71 PROBE OK/)
  && Object.keys(left).length === 1 && Object.values(left).every((v) => !v.manifests.includes("unfinal"));
const res = { ok, run, from: t0.toISOString(), to: new Date().toISOString(), repo: repoId, per, left };
row(join(out, "result.jsonl"), res);
log(`BF-71 ${ok ? "PASS" : "FAIL"}: ${JSON.stringify(res).slice(0, 1500)}`);
log(`sheds: python3 tests/fanout/tieroff503.py ${res.from} ${res.to} graft-m4a-edge`);
for (const s of slots) if (s.child && s.child.exitCode === null) try { process.kill(-s.child.pid, "SIGTERM"); } catch {}
process.exit(ok ? 0 : 1);
