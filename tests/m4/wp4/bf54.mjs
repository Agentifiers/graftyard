// BF-54 on graft-m4a (build/BUILD-PLAN.md, M4-WP3's row, run here on Sandboxes): 2,500 unmodified `actions/runner`
// listeners hold polls on one broker scope (`BROKER_HELD_MAX` 2,000 plus 500) while jobs are queued. Pass: polls past
// the cap get 503; those runners re-poll within 30 s with their sessions valid; job start p99 at most 35 s for them.
// Fail: any `RunnerSessionInvalid`, a runner that exits without its job, or a job unstarted after 60 s.
//
//   source tests/am1/lf13/env-m4a.sh
//   BOX_URL=https://graft-runners-bf54.$CF_RUNNER_WORKERS_SUBDOMAIN.workers.dev BOX_TOKEN_FILE=... \
//   BROKER_URL=https://graft-m4a-memberkill.$CF_WORKERS_SUBDOMAIN.workers.dev BROKER_TOKEN_FILE=... \
//   node tests/m4/wp4/bf54.mjs <out dir>
//
// Shape (tests/m4/results/m4-wp4.md, "BF-54"): `BOXES` (25) `standard-4` Sandboxes of the runner image from the scratch
// Worker `tests/m4/wp4/bf54-box`, `PER` (100) listeners each, every one an ephemeral `Repo`-scope runner of
// `m3org/wp4-bf54` with labels `self-hosted, linux, x64, bf54`, registered through the admin route at `REG_PER_S`
// (10) a second before any Sandbox starts. Each Sandbox then starts its listeners one per `GAP_S` (2.5) s from
// hardlinked copies of `/home/runner`, so all 25 open sessions at about 10 a second. Once `HOLD_MIN` (5) minutes have
// passed with every listener started, a stream of short runs queues jobs (below). Observed: the broker's held polls (`/stats` through the install's scratch `BROKER_<n>` Worker) and each
// Sandbox's memory and listener count every 30 s; every job's queue and start times every 3 s; and at the end every
// listener's `_diag` lines for failed polls (a 503 is `Attempt N of GET .../broker/message ... failed (HTTP Status:
// ServiceUnavailable). The operation will be retried in S seconds.`; a successful poll is not logged), session errors,
// its job, and its exit, which settle the row per runner.
// The keys and JIT configs stay in `KEYS` (/tmp/bf54-keys) and the Sandboxes; nothing secret is written to `<out>`.
import { execFile, execFileSync } from "node:child_process";
import { cpus } from "node:os";
import { promisify } from "node:util";
import { createHash } from "node:crypto";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Git, api as api1, log, row, secret, sleep, tokens } from "../../am1/hosted/lib.mjs";

const out = resolve(process.argv[2] ?? "tests/m4/raw/wp4/bf54");
mkdirSync(out, { recursive: true });
const env = (k, d) => process.env[k] ?? d;
const BOXES = Number(env("BOXES", 25)), PER = Number(env("PER", 100)), RUNS = Number(env("RUNS", 32));
const REG_PER_S = Number(env("REG_PER_S", 10)), GAP_S = Number(env("GAP_S", 2.5)), HOLD_MIN = Number(env("HOLD_MIN", 5));
const NAME = env("BF54_REPO", "wp4-bf54"), R = `m3org/${NAME}`, ORG = 7100001;
const WORKFLOW = env("WORKFLOW", "tests/m4/wp4/hosted/bf54.yml");
const KEYS = env("KEYS", "/tmp/bf54-keys");
const BOX = env("BOX_URL", `https://graft-runners-bf54.${process.env.CF_RUNNER_WORKERS_SUBDOMAIN}.workers.dev`);
const BOX_T = secret(readFileSync(env("BOX_TOKEN_FILE", "/tmp/bf54-box.token"), "utf8").trim());
// Optional: the broker's `/stats` through a scratch Worker bound to the install's `BROKER_<n>` classes. Without it the
// cap shows only in the listeners' own 503s.
const BROKER = process.env.BROKER_URL;
const BROKER_T = BROKER ? secret(readFileSync(env("BROKER_TOKEN_FILE", `${process.env.HOME}/.graft-m4a/memberkill-token`), "utf8").trim()) : null;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const jit = env("JITCONFIG", join(repoRoot, "target/release/examples/jitconfig"));
const N = BOXES * PER;
mkdirSync(KEYS, { recursive: true });
const samples = join(out, "samples.jsonl");
const result = { repo: R, boxes: BOXES, per: PER, runners: N, runs: [], events: [] };
const event = (what, extra = {}) => { result.events.push({ at: Date.now(), what, ...extra }); log(`${what} ${JSON.stringify(extra)}`); };
const save = () => writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));

async function api(...a) {
  for (let i = 0; ; i++) {
    const r = await api1(...a);
    if (r.status !== 503 || i >= 5) return r;
    await sleep(3000);
  }
}
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 300)}`);
  return r.body;
};
/** One call to the scratch box Worker; `body` is a string. */
async function box(path, name, body = "", q = "") {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(`${BOX}${path}?box=${name}${q}`, { method: "POST", headers: { "x-t": BOX_T }, body, signal: AbortSignal.timeout(120_000) });
      const v = await r.json().catch(() => ({ error: `status ${r.status}` }));
      if (r.ok || i >= 4) return v;
    } catch (e) { if (i >= 4) return { error: String(e).slice(0, 200) }; }
    await sleep(5000 * (i + 1));
  }
}
const sh = async (name, cmd) => box("/exec", name, cmd);
async function brokerStats(site, name) {
  if (!BROKER) return null;
  try {
    const r = await fetch(`${BROKER}/?site=${site}&name=${encodeURIComponent(name)}&path=/stats&class=BROKER`, {
      method: "POST", headers: { "x-t": BROKER_T }, body: "{}", signal: AbortSignal.timeout(10_000),
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

// ---- setup: the repository and workflow
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const shard = actionsShard(repoId);
if (shard === 0) throw new Error("the repository hashes to shard 0, the owner's registration shard; pick another name");
const chain = `actions:${ORG}.${shard}`, brokerName = `broker:${chain}`;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bf54.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, WORKFLOW)));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const A = `/api/graftyard/v1/repos/${R}/actions`;
Object.assign(result, { repoId, chain });
event("setup", { repoId, chain, N });

// ---- registration, paced, before any Sandbox starts
const names = [...Array(N).keys()].map((i) => `bf54-${String(i).padStart(4, "0")}`);
const ids = {};
// Keys first, one process per core: `keygen` is 0.8 s of RSA, and run in line it held the pilot to 0.5 registrations a
// second (tests/m4/results/m4-wp4.md, "BF-54").
const keys = {};
const tk = Date.now();
await Promise.all([...Array(cpus().length).keys()].map(async (w) => {
  for (let i = w; i < N; i += cpus().length) {
    const dir = join(KEYS, names[i]);
    mkdirSync(dir, { recursive: true });
    keys[names[i]] = (await promisify(execFile)(jit, ["keygen", dir])).stdout.trim();
  }
}));
event("keys", { seconds: Math.round((Date.now() - tk) / 1000) });
const t0 = Date.now();
let regFailed = 0;
const W = Number(env("REG_WORKERS", 32));
await Promise.all([...Array(W).keys()].map(async (w) => {
  for (let i = w; i < N; i += W) {
    const due = t0 + (i * 1000) / REG_PER_S;
    if (due > Date.now()) await sleep(due - Date.now());
    const dir = join(KEYS, names[i]);
    const key = keys[names[i]];
    const r = await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64", "bf54"], key, name: names[i] });
    if (r.status !== 201) { regFailed += 1; appendFileSync(join(out, "register-errors.log"), `${names[i]} ${r.status} ${JSON.stringify(r.body).slice(0, 200)}\n`); continue; }
    ids[names[i]] = r.body.runner;
    writeFileSync(join(dir, "answer.json"), JSON.stringify(r.body));
  }
}));
const registered = Object.keys(ids).length;
event("registered", { registered, failed: regFailed, seconds: Math.round((Date.now() - t0) / 1000) });
writeFileSync(join(out, "runner-ids.json"), JSON.stringify(ids));
if (registered < N * 0.98) { event("too many registrations failed; stopping before any Sandbox starts"); save(); process.exit(1); }

// ---- the Sandboxes: configs written, then each starts its listeners one per GAP_S
const launch = `set -u
mkdir -p /r/run
for f in /r/cfg/*; do
  n=$(basename "$f"); d=/r/run/$n
  mkdir -p "$d"; cp -al /home/runner/. "$d"/ 2>/dev/null || cp -a /home/runner/. "$d"/
  ( cd "$d" && ./run.sh --jitconfig "$(cat "$f")" > "/r/run/$n.out" 2>&1; echo "exit $? $(date +%s)" >> "/r/run/$n.out" ) &
  sleep ${GAP_S}
done
wait`;
const boxes = [...Array(BOXES).keys()].map((b) => `bf54-box-${b}`);
const tBoxes = Date.now();
await Promise.all(boxes.map(async (bx, b) => {
  const mine = names.slice(b * PER, (b + 1) * PER).filter((n) => ids[n]);
  for (const n of mine) {
    const dir = join(KEYS, n);
    const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), n]).toString();
    const w = await box("/write", bx, config, `&path=/r/cfg/${n}`);
    if (!w.ok) { event("write failed", { box: bx, runner: n, error: w.error }); break; }
  }
  await box("/write", bx, launch, "&path=/r/launch.sh");
  const s = await box("/start", bx, "bash /r/launch.sh > /r/launch.log 2>&1", "&id=launch");
  event("box started", { box: bx, runners: mine.length, ok: !!s.ok, error: s.error });
}));
result.boxesUpS = Math.round((Date.now() - tBoxes) / 1000);
save();

// ---- observe: held polls, box memory and listeners, every 30 s; jobs every 3 s
const boxState = async () => Promise.all(boxes.map(async (bx) => {
  const r = await sh(bx, `free -m | awk '/Mem/{print $3, $2}'; ls /r/run/*.out 2>/dev/null | wc -l; grep -l '^exit' /r/run/*.out 2>/dev/null | wc -l; pgrep -fc Runner.Listener; pgrep -fc Runner.Worker`);
  const [mem, outs, exits, listeners, workers] = (r.stdout ?? "").trim().split("\n");
  const [used, total] = (mem ?? "").split(" ").map(Number);
  return { box: bx, usedMiB: used, totalMiB: total, started: Number(outs), exited: Number(exits), listeners: Number(listeners), workers: Number(workers) };
}));
let maxHeld = 0, startedAll = null, maxUsed = 0;
async function observe() {
  const [held, bs] = await Promise.all([Promise.all([0, 1, 2].map((s) => brokerStats(s, brokerName))), boxState()]);
  const h = Math.max(0, ...held.map((x) => x?.held ?? 0));
  maxHeld = Math.max(maxHeld, h);
  const started = bs.reduce((a, x) => a + (x.started || 0), 0), listeners = bs.reduce((a, x) => a + (x.listeners || 0), 0);
  const exited = bs.reduce((a, x) => a + (x.exited || 0), 0), workers = bs.reduce((a, x) => a + (x.workers || 0), 0);
  maxUsed = Math.max(maxUsed, ...bs.map((x) => x.usedMiB || 0));
  if (!startedAll && started >= registered) startedAll = Date.now();
  row(samples, { t: Date.now(), held: held.map((x) => x && { held: x.held, polls: x.polls }), started, listeners, exited, workers, maxUsedMiB: Math.max(...bs.map((x) => x.usedMiB || 0)), boxes: bs });
  log(`held ${h} (max ${maxHeld}); listeners ${listeners} of ${started} started, ${exited} exited, ${workers} workers; box memory max ${maxUsed} MiB`);
}
const deadline = Date.now() + 25 * 60_000;
while (!startedAll && Date.now() < deadline) { await observe(); await sleep(30_000); }
event("every listener started", { at: startedAll, maxHeld });
const holdEnd = Date.now() + HOLD_MIN * 60_000;
while (Date.now() < holdEnd) { await observe(); await sleep(30_000); }
event("held", { maxHeld });

// ---- the jobs: a steady stream of `RUNS` (32) runs of `WORKFLOW` (a 4 x 4 matrix of 20 s jobs), one every
// `RUN_EVERY_S` (15), about a job a second for 8 minutes. One 256-job run at once would time the shard's commits (each
// job's match, acquire, renews, and finish), not the polls; a stream under that rate leaves every job to the held polls,
// and keeps each Sandbox under its memory with a few workers at a time beside 100 listeners (pilot: 88 MB a listener).
const RUN_EVERY_S = Number(env("RUN_EVERY_S", 15));
const jobs = new Map(); // id -> {run, queued, started, runner, done, conclusion}
const runs = new Map(); // run -> dispatched at
const tRuns = Date.now();
let nextObs = Date.now() + 30_000, k = 0;
for (;;) {
  if (k < RUNS && Date.now() >= tRuns + k * RUN_EVERY_S * 1000) {
    const d = await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" });
    if (d.status === 201) runs.set(d.body.run, Date.now()); else event("dispatch failed", { status: d.status, body: JSON.stringify(d.body).slice(0, 200) });
    k += 1;
  }
  const now = Date.now();
  for (const [run, queued] of runs) {
    if ([...jobs.values()].some((e) => e.run === run) && [...jobs.values()].filter((e) => e.run === run).every((e) => e.done)) continue;
    const r = await api("GET", `${A}/runs/${run}`, admin);
    if (r.status !== 200) continue;
    for (const j of r.body.value.jobs) {
      const e = jobs.get(j.spec.id) ?? { run, queued };
      const st = j.state, runner = st.Assigned?.runner ?? st.InProgress?.runner;
      if (runner != null && !e.started) { e.started = Date.now(); e.runner = runner; }
      if (st.Completed && !e.done) { e.done = Date.now(); e.conclusion = st.Completed.conclusion; e.started ??= e.done; }
      jobs.set(j.spec.id, e);
    }
  }
  const all = [...jobs.values()];
  if (k >= RUNS && all.length >= runs.size * 16 && all.every((e) => e.done)) break;
  if (all.some((e) => !e.started && now - e.queued > 15 * 60_000)) { event("a job unstarted 15 min; ending the wait"); break; }
  if (now >= nextObs) { nextObs = now + 30_000; await observe(); }
  await sleep(2000);
}
result.runs = [...runs.keys()];
event("runs finished", { runs: runs.size, jobs: jobs.size });
save();
await sleep(3 * 60_000);
await observe();

// ---- every listener's own record: polls, 503s, session errors, its job, its exit
const summarize = `cd /r/run 2>/dev/null || exit 0
for d in bf54-*; do
  [ -d "$d" ] || continue
  out="$d.out"
  ex=$(grep '^exit' "$out" 2>/dev/null | tail -1)
  job=$(grep -c 'Running job' "$out" 2>/dev/null)
  res=$(grep -o 'completed with result: [A-Za-z]*' "$out" 2>/dev/null | tail -1 | awk '{print $4}')
  diag=$(ls "$d"/_diag/Runner_*.log 2>/dev/null)
  if [ -n "$diag" ]; then
    grep -hE 'ServiceUnavailable|recovered by recreate session|broker/message|Back off' $diag | \
      sed -E 's/^\\[([0-9-]+ [0-9:]+)Z? ([A-Z]+) +([A-Za-z]+)\\] (.*)$/\\1|\\2|\\3|\\4/' | sed -E 's/\\?sessionId=[^ ]*//' | cut -c1-220 | sed "s/^/$d|/" > "/r/diag-$d.txt"
    p=$(grep -c 'broker/message' "/r/diag-$d.txt"); s5=$(grep -c 'ServiceUnavailable' "/r/diag-$d.txt"); inv=$(grep -c 'recovered by recreate session' "/r/diag-$d.txt")
  else p=0; s5=0; inv=0; fi
  rs=$(grep -c 'Restarting runner' "$out" 2>/dev/null)
  echo "$d	$job	$res	$p	$s5	$inv	$ex	$rs"
done`;
const per = [];
for (const bx of boxes) {
  await box("/write", bx, summarize, "&path=/r/summarize.sh");
  const r = await sh(bx, "bash /r/summarize.sh");
  for (const line of (r.stdout ?? "").trim().split("\n").filter(Boolean)) {
    const [name, job, res, polls, s503, invalid, exit, restarts] = line.split("\t");
    per.push({ box: bx, name, runner: ids[name], jobs: Number(job), result: res || null, failedPolls: Number(polls), s503: Number(s503), invalid: Number(invalid), exit: exit || null, restarts: Number(restarts) || 0 });
  }
  // The 503 and poll lines with times, for the re-poll check, from runners that saw a 503 (at most 60 lines each).
  const d = await sh(bx, `for f in /r/diag-*.txt; do grep -q 'ServiceUnavailable' "$f" && head -60 "$f"; done`);
  writeFileSync(join(out, `diag-${bx}.txt`), d.stdout ?? "");
}
writeFileSync(join(out, "runners.json"), JSON.stringify(per, null, 1));

// ---- the verdict
const byRunner = new Map([...jobs.values()].filter((e) => e.runner != null).map((e) => [e.runner, e]));
const saw503 = per.filter((p) => p.s503 > 0);
const lat = (xs) => { const v = xs.sort((a, b) => a - b); return { n: v.length, p50: v[Math.floor(v.length * 0.5)] ?? null, p99: v[Math.min(v.length - 1, Math.floor(v.length * 0.99))] ?? null, max: v.at(-1) ?? null }; };
const startS = (e) => (e.started - e.queued) / 1000;
const theirs = saw503.map((p) => byRunner.get(p.runner)).filter(Boolean);
const allJobs = [...jobs.values()];
const exitedWithoutJob = per.filter((p) => p.exit && p.jobs === 0);
Object.assign(result, {
  registered, regFailed, maxHeld, maxBoxMiB: maxUsed, startedAll,
  saw503: saw503.length, invalid: per.reduce((a, p) => a + p.invalid, 0), exitedWithoutJob: exitedWithoutJob.map((p) => p.name),
  jobStart: lat(allJobs.filter((e) => e.started).map(startS)), jobStartFor503Runners: lat(theirs.map(startS)),
  unstarted60s: allJobs.filter((e) => !e.started || startS(e) > 60).length,
  jobsFailed: allJobs.filter((e) => e.conclusion !== "Success").length, jobsTotal: allJobs.length,
});
result.checks = {
  polls503PastCap: saw503.length > 0 && (!BROKER || maxHeld >= 2000),
  noSessionInvalid: result.invalid === 0,
  noRunnerExitedWithoutJob: exitedWithoutJob.length === 0,
  noListenerRestart: per.every((p) => !p.restarts),
  jobStartP99Under35sFor503Runners: theirs.length > 0 && result.jobStartFor503Runners.p99 <= 35,
  noJobUnstartedAfter60s: result.unstarted60s === 0,
  rePollWithin30s: "from diag-*.txt (tests/m4/wp4/bf54-repoll.mjs)",
};
save();
log(`BF-54 ${JSON.stringify(result.checks)}; held max ${maxHeld}; 503 runners ${saw503.length}; job start ${JSON.stringify(result.jobStart)}; for 503 runners ${JSON.stringify(result.jobStartFor503Runners)}`);
event("observation done; tear down with tests/m4/wp4/bf54-teardown.mjs");
save();
