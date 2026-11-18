// BF-56 on a hosted install: kill the repository's commit path, and separately `RunOwner`, between `RunCreated` and
// the `RequiredCheck` writes, then finish or cancel every job; every `RequiredCheck` on an open PR head must have a
// conclusion within 60 s of its job's `Completed`.
//   source tests/am1/lf13/env-m4a.sh
//   MEMBERKILL=https://graft-m4a-memberkill... node tests/m4/wp3/bf56.mjs <out dir> <commit|owner> <runner dir>...
// Setup, once: `m3org/wp3-bf56` with `tests/m4/wp3/hosted/bf56.yml`, a branch rule on `main` (GHES `PUT
// .../branches/main/protection`) requiring all six job names, and branches `a` and `b` with an open pull request each.
// Each phase pushes a new commit to `a` and `b`, so its runs' heads are fresh, then:
//   commit  holds `RunOwner` on the shard leader (`/owner_fault {hold_ms}`), dispatches run A on `a` and run B on `b`
//           (`RunCreated`), checks no check run reached the repository, aborts the repository's leader member
//           (`/abort`), waits until the group has no leader, and lifts the hold, so `RunOwner`'s first round meets a
//           dead commit path.
//   owner   arms `/owner_fault {kill}` on the shard leader, then dispatches A and B: the first round that writes checks
//           resets the leader after its repository half and before its cursor, and a new leader runs it again.
// Both phases cancel run B (GHES `POST .../actions/runs/{id}/cancel`) once one of its jobs runs, run every job on
// unmodified ephemeral runners (one per runner dir at a time), and poll the repository leader's `required_check` rows
// every 2 s, timing each conclusion on Cloudflare's clock (the members' `/status` `now`). The kills go through
// `tests/m4/wp3/bf56-kill`, a scratch Worker bound to the `Member` classes; the edge forwards neither route.
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, log, member, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const [out, phase, ...runnerDirs] = process.argv.slice(2);
const NAME = "wp3-bf56";
const R = `m3org/${NAME}`;
const ORG = 7100001;
const JOBS = ["build", "test", "lint", "deploy", "report", "notify"];
const admin = tokens.admin;
const KILL = process.env.MEMBERKILL ?? `https://graft-m4a-memberkill.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const KILL_TOKEN = readFileSync(`${process.env.HOME}/.graft-m4a/memberkill-token`, "utf8").trim();
const repoRoot = new URL("../../..", import.meta.url).pathname;
const jit = join(repoRoot, "target/release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const polls = join(out, `${phase}.jsonl`);
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};
const gql = async (query, variables) => must("graphql", await api("POST", "/api/graphql", admin, { query, variables }));

// ---- the members: status, the leader, the clock, and the kills
async function statusOf(chain, site) {
  try { return (await member(chain, site, "/status", undefined, 15_000)).v; } catch { return null; }
}
async function leader(chain) {
  const st = (await Promise.all([0, 1, 2].map((s) => statusOf(chain, s)))).filter(Boolean);
  const l = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  return l ? { site: l.site, do_name: l.doName, term: l.term, commit: l.commit } : null;
}
/** Cloudflare's clock minus the orb's, from a member's `/status` `now` at the midpoint of the call. */
async function clockOffset(chain) {
  const xs = [];
  for (let i = 0; i < 3; i++) {
    const t0 = Date.now();
    const s = await statusOf(chain, 0);
    const t1 = Date.now();
    if (s?.now) xs.push(s.now - (t0 + t1) / 2);
  }
  xs.sort((a, b) => a - b);
  return xs[Math.floor(xs.length / 2)] ?? 0;
}
async function kill(site, name, path, body = {}) {
  const r = await fetch(`${KILL}/?site=${site}&name=${encodeURIComponent(name)}&path=${encodeURIComponent(path)}`, {
    method: "POST", headers: { "x-t": KILL_TOKEN }, body: JSON.stringify(body),
  });
  const v = await r.json();
  log(`${path} on ${name} (site ${site}): ${JSON.stringify(v).slice(0, 200)}`);
  return v;
}
function actionsShard(repoId) {
  const b = Buffer.alloc(8);
  b.writeBigInt64BE(BigInt(repoId));
  return createHash("sha256").update(b).digest()[0] % 16;
}

// ---- `required_check` rows: key [sha, name, source], one postcard `RequiredRow` blob
function varint(b, at) {
  let v = 0n, s = 0n;
  for (;;) { const x = b[at.i++]; v |= BigInt(x & 0x7f) << s; if (!(x & 0x80)) return v; s += 7n; }
}
const zz = (v) => (v >> 1n) ^ -(v & 1n);
const CONCL = ["action_required", "cancelled", "failure", "neutral", "success", "skipped", "stale", "timed_out", "startup_failure"];
function requiredRow(bytes) {
  const at = { i: 0 };
  const kind = Number(varint(bytes, at));
  const source = kind === 0 ? { app: Number(zz(varint(bytes, at))), run: Number(zz(varint(bytes, at))) } : { creator: Number(zz(varint(bytes, at))) };
  const version = Number(varint(bytes, at));
  const conclusion = bytes[at.i++] === 1 ? CONCL[Number(varint(bytes, at))] : null;
  return { source, version, conclusion };
}
const hex = (blob) => Buffer.from(blob).toString("hex");
async function requiredRows(chain, names) {
  for (const s of [0, 1, 2]) {
    let r;
    try { r = await member(chain, s, "/read", { q: { rows: true } }, 30_000, names[s]); } catch { continue; }
    const core = r?.v?.value?.core;
    if (!core) continue;
    return core.filter((x) => x[0] === "required_check").map(([, k, cols]) => ({ sha: hex(k[0].Blob), name: k[1].Text, ...requiredRow(cols[0].Blob) }));
  }
  return null;
}

// ---- setup
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const repoChain = `repo:${repoId}`;
const shardChain = `actions:${ORG}.${actionsShard(repoId)}`;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bf56.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp3/hosted/bf56.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const rule = await api("PUT", `/api/v3/repos/${R}/branches/main/protection`, admin, {
  required_status_checks: { strict: false, contexts: JOBS }, enforce_admins: false, required_pull_request_reviews: null, restrictions: null,
});
must("protection", rule, 200);
log(`rule on main: ${JSON.stringify(rule.body.required_status_checks.contexts)}`);
const heads = {};
for (const br of ["a", "b"]) {
  g.run(["checkout", "-q", "-B", br, "origin/main"]);
  g.run(["fetch", "-q", "origin", br]);
  if (g.run(["rev-parse", "-q", "--verify", `origin/${br}`]).ok) g.run(["reset", "-q", "--hard", `origin/${br}`]);
  writeFileSync(join(g.wt, `${br}.txt`), `${phase} ${new Date().toISOString()}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `BF-56 ${phase} on ${br}`]);
  if (!g.run(["push", "-q", "origin", `HEAD:refs/heads/${br}`]).ok) throw new Error(`push ${br}`);
  heads[br] = g.run(["rev-parse", "HEAD"]).out.trim();
}
const repoNode = (await gql(`query { repository(owner: "m3org", name: "${NAME}") { id pullRequests(states: OPEN, first: 10) { nodes { id number headRefName } } } }`)).data.repository;
const prs = {};
for (const n of repoNode.pullRequests.nodes) prs[n.headRefName] = n;
for (const br of ["a", "b"]) {
  if (prs[br]) continue;
  const c = await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number headRefName } } }", {
    i: { repositoryId: repoNode.id, baseRefName: "main", headRefName: br, title: `BF-56 ${br}`, body: "BF-56's required checks." },
  });
  prs[br] = c.data.createPullRequest.pullRequest;
}
log(`repo ${R} (${repoChain}), shard ${shardChain}, heads ${JSON.stringify(heads)}, PRs ${prs.a.number} and ${prs.b.number}`);
const names = Object.fromEntries((await statusOf(repoChain, 0)).config.config.normal.voters.map((v) => [v.site, v.do_name]));
const offset = await clockOffset(repoChain);
const cf = () => Date.now() + offset;
log(`Cloudflare's clock is ${Math.round(offset)} ms ahead of the orb's`);

// ---- the phase's kill, and the runs
const A = `/api/graftyard/v1/repos/${R}/actions`;
const result = { phase, repo: R, repoChain, shardChain, heads, prs: { a: prs.a.number, b: prs.b.number }, offsetMs: Math.round(offset), events: [] };
const event = (what, extra = {}) => { const e = { at: cf(), what, ...extra }; result.events.push(e); log(`${what} ${JSON.stringify(extra)}`); };
// The shard is created on the repository's first Actions call; a read of a run that does not exist creates it.
await api("GET", `${A}/runs/0`, admin);
let shardLeader = null;
for (let i = 0; i < 30 && !shardLeader; i++) {
  shardLeader = await leader(shardChain);
  if (!shardLeader) await sleep(1000);
}
if (!shardLeader) throw new Error(`no leader for ${shardChain}`);
if (phase === "commit") {
  await kill(shardLeader.site, shardLeader.do_name, "/owner_fault", { hold_ms: 180_000 });
  event("RunOwner held", { shardLeader });
} else {
  await kill(shardLeader.site, shardLeader.do_name, "/owner_fault", { kill: true });
  event("RunOwner kill armed", { shardLeader });
}
const dispatch = async (br) => must(`dispatch ${br}`, await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: br }), 201).run;
const runA = await dispatch("a");
// A write between the two, so the second dispatch's trigger index (its run key) differs from the first's.
await gql("mutation($i: AddCommentInput!) { addComment(input: $i) { clientMutationId } }", { i: { subjectId: prs.a.id, body: `BF-56 ${phase}: run ${runA}` } });
const runB = await dispatch("b");
event("RunCreated", { runA, runB });
if (phase === "commit") {
  const before = (await requiredRows(repoChain, names)) ?? [];
  const runs = must("check-runs", await api("GET", `/api/v3/repos/${R}/commits/${heads.a}/check-runs`, admin)).total_count;
  event("before the kill", { requiredRowsForHeads: before.filter((x) => x.sha === heads.a || x.sha === heads.b).length, checkRunsOnA: runs });
  const l = await leader(repoChain);
  await kill(l.site, l.do_name, "/abort");
  event("repository leader aborted", { leader: l });
  for (let i = 0; i < 20; i++) {
    const now = await leader(repoChain);
    if (!now || now.term === l.term && now.site !== l.site) { event("repository group leaderless", { seen: now }); break; }
    if (now.term > l.term) { event("repository group re-elected before a leaderless read", { now }); break; }
    await sleep(500);
  }
  await kill(shardLeader.site, shardLeader.do_name, "/owner_fault", { hold_ms: 0 });
  event("RunOwner hold lifted");
}

// ---- runners, polls, the cancel
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
async function startRunner(slot) {
  n += 1;
  const dir = join(out, `${phase}-runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf56-${phase}-${n}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf56-${phase}-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slot.dir, f), { force: true });
  rmSync(join(slot.dir, "_diag"), { recursive: true, force: true });
  rmSync(join(slot.dir, "_work"), { recursive: true, force: true });
  const o = join(out, `${phase}-runner-${n}.out`);
  slot.child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`], { cwd: slot.dir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  slot.child.stdin.end(config);
  log(`runner ${n} (id ${reg.runner}) in ${slot.dir}`);
}
const jobsOf = async (run) => {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  if (r.status !== 200) return null;
  return r.body.value.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
};
const seen = {}; // `${sha}/${name}` -> { conclusion, at } the first poll that saw a conclusion, Cloudflare's clock
const firstNone = {};
let cancelled = null;
let allDoneAt = null;
let lastShard = shardLeader;
const t0 = Date.now();
for (;;) {
  const [ja, jb] = [await jobsOf(runA), await jobsOf(runB)];
  const rows = await requiredRows(repoChain, names);
  const now = cf();
  if (rows) {
    for (const x of rows) {
      const k = `${x.sha}/${x.name}`;
      if (x.conclusion && !seen[k]) seen[k] = { conclusion: x.conclusion, at: now, version: x.version, run: x.source.run };
      if (!x.conclusion && !firstNone[k]) firstNone[k] = now;
    }
  }
  const sl = await leader(shardChain);
  if (sl && (sl.site !== lastShard?.site || sl.term !== lastShard?.term)) { event("shard leader", { leader: sl }); lastShard = sl; }
  if (!sl && lastShard) { event("shard leaderless"); lastShard = null; }
  row(polls, { t: now, a: ja?.map((j) => [j.name, j.state]), b: jb?.map((j) => [j.name, j.state]), rows: rows?.length ?? null });
  if (!cancelled && jb?.some((j) => j.state === "InProgress")) {
    const c = await api("POST", `/api/v3/repos/${R}/actions/runs/${runB}/cancel`, admin);
    cancelled = { at: cf(), status: c.status, body: c.body, running: jb.filter((j) => j.state === "InProgress").map((j) => j.name) };
    event("cancel run B", cancelled);
  }
  const all = [...(ja ?? []), ...(jb ?? [])];
  if (ja && jb && all.every((j) => j.state === "Completed")) {
    allDoneAt ??= now;
    const last = Math.max(...all.map((j) => j.s.Completed.at));
    const missing = [...ja.map((j) => [heads.a, j]), ...jb.map((j) => [heads.b, j])].filter(([sha, j]) => !seen[`${sha}/${j.name}`]);
    if (missing.length === 0 || now > last + 90_000) break;
  }
  for (const slot of slots) {
    const busy = slot.child && slot.child.exitCode === null;
    if (!busy && all.some((j) => j.state === "Queued" || j.state === "Pending")) await startRunner(slot);
  }
  if (Date.now() - t0 > 30 * 60_000) { event("gave up after 30 min"); break; }
  await sleep(2000);
}
for (const slot of slots) if (slot.child && slot.child.exitCode === null) try { process.kill(-slot.child.pid, "SIGKILL"); } catch {}

// ---- the verdict
const [ja, jb] = [await jobsOf(runA), await jobsOf(runB)];
const finalRows = (await requiredRows(repoChain, names)) ?? [];
const want = { build: "success", test: "success", lint: "failure", deploy: "success", report: "skipped", notify: "skipped" };
const lower = (c) => c.replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase();
const jobs = [];
for (const [run, sha, list] of [["A", heads.a, ja], ["B", heads.b, jb]]) {
  for (const j of list) {
    const k = `${sha}/${j.name}`;
    const done = j.s.Completed;
    const s = seen[k];
    const fin = finalRows.find((x) => x.sha === sha && x.name === j.name);
    jobs.push({
      run, name: j.name, job: j.id, conclusion: done && lower(done.conclusion), completedAt: done?.at,
      required: fin ? { conclusion: fin.conclusion, version: fin.version, run: fin.source.run } : null,
      seenAt: s?.at ?? null, lagS: s && done ? Math.round((s.at - done.at) / 100) / 10 : null, firstPendingAt: firstNone[k] ?? null,
    });
  }
}
const checks = {
  everyJobCompleted: jobs.every((j) => j.completedAt),
  runA: jobs.filter((j) => j.run === "A").every((j) => j.conclusion === want[j.name]),
  runBCancelledRest: jobs.filter((j) => j.run === "B").some((j) => j.conclusion === "cancelled"),
  everyRequiredConcluded: jobs.every((j) => j.required?.conclusion),
  requiredMatchesJob: jobs.every((j) => j.required?.conclusion === j.conclusion && j.required?.run === j.job),
  within60s: jobs.every((j) => j.lagS !== null && j.lagS <= 60),
  noPendingOnHeads: finalRows.filter((x) => x.sha === heads.a || x.sha === heads.b).every((x) => x.conclusion),
};
Object.assign(result, { runA, runB, cancelled, jobs, checks, maxLagS: Math.max(...jobs.map((j) => j.lagS ?? Infinity)), pass: Object.values(checks).every(Boolean) });
writeFileSync(join(out, `${phase}.json`), JSON.stringify(result, null, 2));
log(`BF-56 ${phase}: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)} max lag ${result.maxLagS} s`);
for (const j of jobs) log(`  ${j.run} ${j.name}: ${j.conclusion}, required ${j.required?.conclusion} after ${j.lagS} s`);
process.exit(result.pass ? 0 : 1);
