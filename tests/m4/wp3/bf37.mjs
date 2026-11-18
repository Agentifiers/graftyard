// BF-37 on a hosted install (`build/BUILD-PLAN.md`, M4-WP3): an `on: merge_group` workflow, and go-playground/webhooks
// parsing `merge_group`, `pull_request.enqueued`, and `pull_request.dequeued`. Pass: a run on each built queue ref, and
// every queue ref deleted within 5 min of landing or ejection. Fail: a missing delivery, or a queue ref left over.
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp3/bf37.mjs <out dir> <runner dir>...
// Setup: `m3org/wp3-bf37` with `tests/m4/wp3/hosted/bf37.yml` (`on: merge_group`, one job `queue-ci` that fails when
// the file at the queue ref's tip sets `BF37_FAIL: "1"`), a rule on `main` requiring `queue-ci` with a merge queue (the branch protection
// `PUT`'s `merge_queue`, ALLGREEN, at most 5 built, one to merge, no wait), and a hook on the repository for
// `merge_group` and `pull_request` to the BF-37 sink (`tests/m4/wp3/bf37/sink`). Three branches, each with an open
// pull request: #A and #C pass, #B sets `BF37_FAIL: "1"`, so its queue ref's run fails and the queue ejects it (`CI_FAILURE`).
// All three are enqueued (GraphQL `enqueuePullRequest`, as gh does). Unmodified ephemeral runners run every job.
// Every 5 s the driver lists the repository's `gh-readonly-queue/*` refs (`git ls-remote`) and its runs (the admin
// `GET .../actions/runs`), on Cloudflare's clock; at the end every delivery the sink kept is parsed by
// go-playground/webhooks (`tests/m4/wp3/bf37/main.go`).
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { Git, api as api1, log, member, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

// A 503 that says "retry" (a new repository's tier rows shedding a read) is retried, as a GHES client would.
async function api(...a) {
  for (let i = 0; ; i++) {
    const r = await api1(...a);
    if (r.status !== 503 || i >= 5) return r;
    await sleep(3000);
  }
}

const [outArg, ...runnerDirs] = process.argv.slice(2);
const out = resolve(outArg);
const NAME = process.env.BF37_REPO || "wp3-bf37";
const R = `m3org/${NAME}`;
const admin = tokens.admin;
const SINK = process.env.BF37_SINK ?? `https://graft-m4a-bf37sink.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const SECRET = `bf37-${Date.now().toString(36)}`;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const jit = join(repoRoot, "target/release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const polls = join(out, "polls.jsonl");
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};
const gql = async (query, variables) => {
  const b = must("graphql", await api("POST", "/api/graphql", admin, { query, variables }));
  if (b.errors) throw new Error(`graphql: ${JSON.stringify(b.errors).slice(0, 400)}`);
  return b;
};

// ---- setup
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
if (!(made.status === 422 && JSON.stringify(made.body).includes("already exists"))) must("create", made, 201);
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const repoChain = `repo:${repoId}`;
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const wf = ".github/workflows/bf37.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp3/hosted/bf37.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");
const rule = must("protection", await api("PUT", `/api/v3/repos/${R}/branches/main/protection`, admin, {
  required_status_checks: { strict: false, contexts: ["queue-ci"] }, enforce_admins: false,
  required_pull_request_reviews: null, restrictions: null,
  merge_queue: { check_response_timeout_minutes: 15, grouping_strategy: "ALLGREEN", max_entries_to_build: 5,
    min_entries_to_merge: 1, max_entries_to_merge: 1, min_entries_to_merge_wait_minutes: 0, merge_method: "MERGE" },
}));
log(`rule on main: ${JSON.stringify(rule.required_status_checks.contexts)}`);
const hooks = must("hooks", await api("GET", `/api/v3/repos/${R}/hooks`, admin));
for (const h of hooks) await api("DELETE", `/api/v3/repos/${R}/hooks/${h.id}`, admin);
await fetch(`${SINK}/clear`);
const hook = must("hook", await api("POST", `/api/v3/repos/${R}/hooks`, admin,
  { config: { url: `${SINK}/h/bf37`, content_type: "json", secret: SECRET }, events: ["merge_group", "pull_request"] }), 201);
log(`hook ${hook.id} to ${SINK}`);

const branches = { a: false, b: true, c: false };
const heads = {};
for (const [br, fail] of Object.entries(branches)) {
  g.run(["checkout", "-q", "-B", br, "origin/main"]);
  g.run(["fetch", "-q", "origin", "main"]);
  g.run(["reset", "-q", "--hard", "origin/main"]);
  writeFileSync(join(g.wt, `${br}.txt`), `${br} ${new Date().toISOString()}\n`);
  if (fail) writeFileSync(join(g.wt, wf), readFileSync(join(g.wt, wf), "utf8").replace('BF37_FAIL: "0"', 'BF37_FAIL: "1"'));
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `BF-37 ${br}`]);
  if (!g.run(["push", "-q", "-f", "origin", `HEAD:refs/heads/${br}`]).ok) throw new Error(`push ${br}`);
  heads[br] = g.run(["rev-parse", "HEAD"]).out.trim();
}
const repoNode = (await gql(`query { repository(owner: "m3org", name: "${NAME}") { id pullRequests(states: OPEN, first: 20) { nodes { id number headRefName headRefOid } } } }`)).data.repository;
const prs = {};
for (const n of repoNode.pullRequests.nodes) prs[n.headRefName] = n;
for (const br of Object.keys(branches)) {
  if (prs[br]) continue;
  const c = await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number headRefName } } }", {
    i: { repositoryId: repoNode.id, baseRefName: "main", headRefName: br, title: `BF-37 ${br}`, body: "BF-37's merge queue." },
  });
  prs[br] = c.data.createPullRequest.pullRequest;
}
log(`repo ${R} (${repoChain}), PRs ${Object.entries(prs).map(([b, p]) => `${b}=#${p.number}`).join(" ")}`);

// Cloudflare's clock, from the repository members' `/status` `now`.
let offset = 0;
{
  const xs = [];
  for (let i = 0; i < 3; i++) {
    const t0 = Date.now();
    const s = (await member(repoChain, i, "/status", undefined, 15_000)).v;
    if (s?.now) xs.push(s.now - (t0 + Date.now()) / 2);
  }
  xs.sort((a, b) => a - b);
  offset = xs[1] ?? xs[0] ?? 0;
}
const cf = () => Date.now() + offset;
log(`Cloudflare's clock is ${Math.round(offset)} ms ahead of the orb's`);
const A = `/api/graftyard/v1/repos/${R}/actions`;
await api("GET", `${A}/runs/0`, admin);
const runsBefore = new Set((must("runs", await api("GET", `${A}/runs`, admin)).runs ?? []).map((r) => r.id));

// ---- enqueue, in order a, b, c
const t0 = cf();
for (const br of ["a", "b", "c"]) {
  await gql("mutation($i: EnqueuePullRequestInput!) { enqueuePullRequest(input: $i) { clientMutationId } }", { i: { pullRequestId: prs[br].id } });
  log(`enqueued #${prs[br].number} (${br})`);
}

// ---- runners and polls
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
async function startRunner(slot) {
  n += 1;
  const dir = join(out, `runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf37-${n}` }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf37-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slot.dir, f), { force: true });
  rmSync(join(slot.dir, "_diag"), { recursive: true, force: true });
  rmSync(join(slot.dir, "_work"), { recursive: true, force: true });
  const o = join(out, `runner-${n}.out`);
  slot.child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`], { cwd: slot.dir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  slot.child.stdin.end(config);
  log(`runner ${n} (id ${reg.runner})`);
}
// The queue refs as a git client sees them: `git ls-remote` over smart HTTP.
const queueRefs = async () => {
  const r = g.run(["ls-remote", g.url, "refs/heads/gh-readonly-queue/*"], g.dir);
  if (!r.ok) return null;
  return r.out.trim().split("\n").filter(Boolean).map((l) => { const [sha, ref] = l.split("\t"); return { ref, sha }; });
};
const refs = {}; // ref -> { sha, firstSeen, goneAt }
const runs = {}; // run id -> { head_ref, firstSeen, jobs }
const pulls = async (br) => (await api("GET", `/api/v3/repos/${R}/pulls/${prs[br].number}`, admin)).body;
let settledAt = null;
for (;;) {
  const now = cf();
  const qr = await queueRefs();
  if (qr) {
    for (const x of qr) refs[x.ref] ??= { sha: x.sha, firstSeen: now, goneAt: null };
    for (const [k, v] of Object.entries(refs)) if (!v.goneAt && !qr.some((x) => x.ref === k)) v.goneAt = now;
  }
  const list = (await api("GET", `${A}/runs`, admin)).body?.runs ?? [];
  for (const r of list.filter((r) => !runsBefore.has(r.id))) runs[r.id] ??= { ...r, firstSeen: now };
  const states = {};
  for (const id of Object.keys(runs)) {
    const v = (await api("GET", `${A}/runs/${id}`, admin)).body?.value;
    runs[id].jobs = (v?.jobs ?? []).map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
    for (const j of runs[id].jobs) states[j.state] = (states[j.state] ?? 0) + 1;
  }
  const merged = {};
  for (const br of Object.keys(branches)) merged[br] = (await pulls(br))?.merged ?? null;
  row(polls, { t: now, queueRefs: qr?.map((x) => x.ref), runs: Object.keys(runs).length, states, merged });
  const pending = Object.values(runs).some((r) => r.jobs.some((j) => j.state !== "Completed"));
  for (const slot of slots) {
    const busy = slot.child && slot.child.exitCode === null;
    if (!busy && Object.values(runs).some((r) => r.jobs.some((j) => j.state === "Queued"))) await startRunner(slot);
  }
  const done = merged.a && merged.c && qr && qr.length === 0 && !pending;
  if (done) { settledAt ??= now; if (now - settledAt > 15_000) break; }
  if (now - t0 > 25 * 60_000) { log("gave up after 25 min"); break; }
  await sleep(5000);
}
for (const slot of slots) if (slot.child && slot.child.exitCode === null) try { process.kill(-slot.child.pid, "SIGKILL"); } catch {}

// ---- deliveries through go-playground/webhooks
await sleep(10_000);
const dump = await (await fetch(`${SINK}/dump`)).json();
const mine = dump.filter((d) => d.hook === String(hook.id));
writeFileSync(join(out, "deliveries.json"), JSON.stringify(mine, null, 1));
let parsed;
try {
  parsed = execFileSync("go", ["run", ".", join(out, "deliveries.json"), SECRET], { cwd: join(repoRoot, "tests/m4/wp3/bf37") }).toString();
} catch (e) {
  parsed = `${e.stdout ?? ""}`;
}
writeFileSync(join(out, "parsed.jsonl"), parsed);
const lines = parsed.trim().split("\n").map((l) => JSON.parse(l));
const summary = lines.pop();
const got = lines.filter((l) => l.ok);
const has = (event, action, pred = () => true) => got.some((l) => l.event === event && l.action === action && pred(l));

// ---- the verdict
const final = await Promise.all(Object.keys(branches).map(async (br) => [br, await pulls(br)]));
const byRef = Object.entries(refs).map(([ref, v]) => {
  const rs = Object.values(runs).filter((r) => r.head_ref === ref);
  const job = rs.flatMap((r) => r.jobs)[0];
  // A ref's run counts once it has a job and that job ran or the ref was gone before it could (a later entry's ref,
  // unbuilt by an ejection ahead of it, is destroyed while its job waits; its run is cancelled or ends on its own).
  return { ref, sha: v.sha, firstSeen: v.firstSeen, goneAt: v.goneAt, runs: rs.map((r) => r.id), event: rs[0]?.event,
    headSha: rs[0]?.head_sha, conclusion: job?.s?.Completed?.conclusion ?? job?.state ?? null,
    checksRequested: has("merge_group", "checks_requested", (l) => l.detail === ref),
    destroyed: has("merge_group", "destroyed", (l) => l.detail === ref) };
});
// When each ref's end came: the landing or ejection that removed it is the poll that first missed it; its deletion is
// in the same entry, so the lag is at most one poll (5 s) plus the read.
const checks = {
  aAndCMerged: final.find(([b]) => b === "a")[1]?.merged === true && final.find(([b]) => b === "c")[1]?.merged === true,
  bNotMerged: final.find(([b]) => b === "b")[1]?.merged === false,
  queueRefsBuilt: byRef.length >= 3,
  aRunOnEachQueueRef: byRef.every((r) => r.runs.length === 1 && r.event === "merge_group" && r.headSha === r.sha),
  everyQueueRefDeleted: byRef.every((r) => r.goneAt),
  noQueueRefLeft: (await queueRefs())?.length === 0,
  checksRequestedForEachRef: byRef.every((r) => r.checksRequested),
  destroyedForEachRef: byRef.every((r) => r.destroyed),
  enqueuedForEach: Object.values(prs).every((p) => has("pull_request", "enqueued", (l) => l.detail === `#${p.number}`)),
  dequeuedForB: has("pull_request", "dequeued", (l) => l.detail === `#${prs.b.number}`),
  everyDeliveryParsed: summary.failed === 0 && summary.deliveries > 0,
};
const result = { repo: R, repoChain, prs: Object.fromEntries(Object.entries(prs).map(([b, p]) => [b, p.number])), heads, offsetMs: Math.round(offset),
  hook: hook.id, refs: byRef, runs: Object.values(runs).map((r) => ({ id: r.id, head_ref: r.head_ref, jobs: r.jobs.map((j) => [j.name, j.state, j.s?.Completed?.conclusion]) })),
  deliveries: summary, checks, pass: Object.values(checks).every(Boolean) };
writeFileSync(join(out, "result.json"), JSON.stringify(result, null, 2));
log(`BF-37: ${result.pass ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
for (const r of byRef) log(`  ${r.ref.split("/").pop()}: runs ${r.runs} ${r.conclusion}, built ${new Date(r.firstSeen).toISOString()}, gone ${r.goneAt && new Date(r.goneAt).toISOString()}`);
log(`  deliveries ${JSON.stringify(summary)}`);
process.exit(result.pass ? 0 : 1);
