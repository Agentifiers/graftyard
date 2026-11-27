// LM-3's L0-04 row on graft-m4a (`build/BUILD-PLAN.md`, L0-04; `decisions/DECISIONS.md`, "`secrets-wait`"; the lead's
// GO, 2026-11-27): agent CI under `ci: AutoReadOnly` with format 0x15.14, and per branch with 0x15.15 (F150). The agents half (both Apps, the controller
// with its `TaskBox` and the kill row's scripted harness, `graft-agents-front`, the probe) is up first
// (`tests/am3/hosted/l004-row.sh`). graft-m4a's sites set GRAFTYARD_PR_TRIGGER, so an agent push's run is the
// `pull_request` run of the task's draft (`refs/heads/graftyard/<task>/work`), which the controller opens at the task's
// first alarm with `AgentMeta` naming m3-admin as the requester (DECISIONS "`a3-open`").
//
//   node tests/am3/hosted/l004.mjs <line> <out dir> <repo name> <state dir>
//
// Lines:
//   repo      m3org/<repo>; the repository secret L004_SECRET (a random value kept 0600 in <state dir>, never printed);
//             `tests/am3/hosted/l004.yml` on main, pushed by m3-admin; m3-member a writer; `ci: auto_read_only`
//   l004      one Graftyard-run task by m3-admin whose harness pushes twice to its work branch and holds. Then:
//             (a) each agent run is fork; `plain` and `missed` start within 30 s with no secret and a read-only token
//                 (`missed` names the secret in a form the scanner does not see, so it runs, with an empty value)
//             (b) `secret` waits (`SecretsWait` on its own head, GHES `waiting`)
//             (c) an approval by the requester, and one by m3-member naming an older head, release nothing for 60 s
//             (s) same SHA (F150): before any approval, m3-admin pushes the agent's current head to a second agent-prefix
//                 branch (`refs/heads/graftyard/l004-same-<t>/work`) and opens a pull request from it; its run is an
//                 agent run (the prefix), so its `secret` waits on the same commit
//             (d) m3-member approves the current head over GraphQL `addPullRequestReview`; `secret` then runs with the
//                 real secret's hash, and its token write is still refused; every other run's `secret` stays waiting,
//                 and the second pull request's `secret`, at the same commit, stays waiting 60 s after the release:
//                 the gate held for the agent's pull request, not for that one. 0x15.14's `ReleaseSecrets` releases it
//                 (the gap F150 closes), so this check passes only where a site day set `GRAFTYARD_SECRETS_BRANCH`
//             control: the same workflow on a branch m3-admin pushes, with a pull request m3-admin opens: not fork,
//             `secret` queues at once and reads the secret
//             Fail: any job of an agent run that read the secret before AgentGate held for its head.
//   teardown  the repository deleted, the policy reset, the value file shredded
//
// Each line appends JSON rows to <out>/<line>.jsonl and prints one `VERDICT <line> PASS|FAIL|INCOMPLETE <why>`. No token
// and no secret value is printed (`lib.mjs`'s `scan`); job logs are saved with the value replaced.
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createHash, randomBytes } from "node:crypto";
import { spawnSync } from "node:child_process";
import { EDGE, Git, HOST, api, log, secret, sleep, tokens } from "../../am1/hosted/lib.mjs";

const [line, out, name, stateDir] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const R = `m3org/${name}`, V3 = `/api/v3/repos/${R}`, V1 = `/api/graftyard/v1/repos/${R}`;
const admin = tokens.admin, member = tokens.member;
const ADMIN_ID = 7100003;
const rows = join(out, `${line}.jsonl`);
const ev = (v) => { appendFileSync(rows, JSON.stringify({ t: Date.now(), ...v }) + "\n"); log(JSON.stringify(v)); };
const fails = [], incomplete = [];
const check = (ok, what) => { ev({ check: what, ok: !!ok }); if (!ok) fails.push(what); };
const gap = (what) => { ev({ incomplete: what }); incomplete.push(what); };
const verdict = () => {
  const v = fails.length ? "FAIL" : incomplete.length ? "INCOMPLETE" : "PASS";
  console.log(`VERDICT ${line} ${v} ${[...fails, ...incomplete.map((x) => `incomplete: ${x}`)].join("; ")}`);
  process.exit(fails.length ? 1 : 0);
};
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
async function until(what, f, ms, every = 5000) {
  const t = Date.now();
  for (;;) {
    const v = await f();
    if (v) return v;
    if (Date.now() - t > ms) { ev({ timeout: what, ms }); return null; }
    await sleep(every);
  }
}
const clone = async () => {
  const g = new Git(R).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
  return g;
};
const VALUE_FILE = join(stateDir, "l004.value");
const value = () => secret(readFileSync(VALUE_FILE, "utf8").trim());
const h16 = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);
const EMPTY = h16("");
const hex = (h) => (Array.isArray(h) ? Buffer.from(h).toString("hex") : typeof h === "string" ? h : null);
const stateOf = (j) => Object.keys(j?.state ?? {})[0];

// ---- runs ----
const runsOf = async (pr) => ((await api("GET", `${V1}/actions/runs`, admin)).body?.runs ?? [])
  .filter((x) => x.event === "pull_request" && x.head_ref === `refs/pull/${pr}/merge`);
/** One run: the shard's row (fork, each job's state and the head a waiting job waits on) and the GHES jobs list. */
async function runView(id) {
  const row = (await api("GET", `${V1}/actions/runs/${id}`, admin)).body?.value;
  const gh = (await api("GET", `${V3}/actions/runs/${id}/jobs`, admin)).body?.jobs ?? [];
  const jobs = (row?.jobs ?? []).map((j) => {
    const g = gh.find((x) => x.id === j.spec?.id) ?? gh.find((x) => x.name === j.spec?.name);
    const st = stateOf(j);
    return { id: j.spec?.id, name: j.spec?.name, state: st, waits_on: st === "SecretsWait" ? hex(j.state.SecretsWait.head) : null,
      // A job with no step report reads its run's `created_at` as `started_at` (`rest::job`), so `steps` says whether
      // a runner ran it.
      status: g?.status, conclusion: g?.conclusion, steps: g?.steps?.length ?? 0, started_at: g?.steps?.length ? g.started_at : null };
  });
  return { id, fork: row?.run?.run?.fork, head_sha: row?.run?.run?.head_sha ? hex(row.run.run.head_sha) : null, jobs };
}
/** A finished job's log: the secret line's hash and length, and the token write's status. Saved with the value cut. */
async function jobLog(id, tag) {
  const r = await fetch(`${EDGE}${V3}/actions/jobs/${id}/logs`, { headers: { authorization: `token ${admin}` }, signal: AbortSignal.timeout(60_000) });
  const text = r.ok ? await r.text() : "";
  const v = value();
  writeFileSync(join(out, `l004-job-${tag}-${id}.log`), text.replaceAll(v, "<secret>"));
  const m = text.match(/secret ([0-9a-f]{16}) (\d+)/), w = text.match(/token write (\d{3})/);
  return { status: r.status, hash: m?.[1] ?? null, len: m ? Number(m[2]) : null, real: m?.[1] === h16(v), empty: m?.[1] === EMPTY,
    raw_value_in_log: text.includes(v), token_write: w ? Number(w[1]) : null };
}
const done = (j) => j?.status === "completed";
async function finished(runId, jobName, ms) {
  return until(`${jobName} of run ${runId} completed`, async () => {
    const j = (await runView(runId)).jobs.find((x) => x.name === jobName);
    return done(j) ? j : null;
  }, ms, 5000);
}

// ---- pull requests ----
const prNode = async (n) => (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { pullRequest(number: ${n}) { id headRefOid isDraft } } }`)).body?.data?.repository?.pullRequest;
const approve = (tok, id, commitOID) => gql(tok, "mutation($i: AddPullRequestReviewInput!) { addPullRequestReview(input: $i) { pullRequestReview { id state } } }",
  { i: { pullRequestId: id, event: "APPROVE", body: "L0-04", ...(commitOID ? { commitOID } : {}) } });

if (line === "repo") {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
  ev({ create: R, status: made.status });
  if (made.status !== 201) process.exit(1);
  if (!existsSync(VALUE_FILE)) writeFileSync(VALUE_FILE, `l004-${randomBytes(18).toString("base64url")}\n`, { mode: 0o600 });
  mkdirSync(join(out, "ghcfg"), { recursive: true });
  const gh = spawnSync("gh", ["secret", "set", "L004_SECRET", "-R", `${HOST}/${R}`], { input: value(), encoding: "utf8",
    env: { ...process.env, GH_HOST: HOST, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" } });
  const sl = await api("GET", `${V3}/actions/secrets`, admin);
  ev({ secret_set: gh.status, gh: String(gh.stderr ?? "").trim().slice(0, 160), secrets: (sl.body?.secrets ?? []).map((x) => x.name), value_hash: h16(value()), value_len: value().length });
  const g = await clone();
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, ".github/workflows/l004.yml"), readFileSync(new URL("./l004.yml", import.meta.url)));
  g.run(["add", "-A"]); g.run(["commit", "-q", "-m", "l004 workflow"]);
  ev({ workflow_main: g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok });
  const inv = await api("PUT", `${V3}/collaborators/m3-member`, admin, { permission: "push" });
  const perm = await api("GET", `${V3}/collaborators/m3-member/permission`, admin);
  ev({ member_invite: inv.status, member_permission: perm.body?.permission, role: perm.body?.role_name });
  const pol = await api("PUT", `${V1}/agents/policy`, admin, { ci: "auto_read_only" });
  ev({ policy: pol.status, ci: pol.body?.ci });
  const ok = (sl.body?.secrets ?? []).some((x) => x.name === "L004_SECRET") && ["write", "admin"].includes(perm.body?.permission) && pol.body?.ci === "auto_read_only";
  console.log(`VERDICT repo ${ok ? "PASS" : "FAIL"}`);
  process.exit(ok ? 0 : 1);
} else if (line === "l004") {
  const g = await clone();
  // ---- the agent's draft ----
  const tc = await api("POST", `${V1}/tasks`, admin, { prompt: "LM-3 L0-04", summary: "l004" });
  ev({ task: tc.status, id: tc.body?.id, requester: tc.body?.requester?.login });
  if (tc.status !== 201) { check(false, "the task is created"); verdict(); }
  const id = tc.body.id, work = `graftyard/${id}/work`;
  g.run(["fetch", "-q", "origin"]); g.run(["checkout", "-q", "--detach", "origin/main"]);
  ev({ hold: g.run(["push", "-q", "origin", `HEAD:refs/heads/hold/${id}`]).ok });
  check(tc.body?.requester?.id === ADMIN_ID, "the task's requester is m3-admin");
  const live = await until("in_progress", async () => (await api("GET", `${V1}/tasks/${encodeURIComponent(id)}`, admin)).body?.state === "in_progress", 8 * 60_000);
  check(!!live, "the task runs");
  // The harness's last push (`push 2`, 15 s after `push 1`; try 1 took `push 1` for the last and approved a head the
  // harness then moved past), and the branch still there 45 s later.
  const tip = () => {
    g.run(["fetch", "-q", "origin", `+refs/heads/${work}:refs/remotes/origin/work`]);
    const s = g.run(["log", "-1", "--format=%H %s", "refs/remotes/origin/work"]);
    return s.ok ? s.out.trim() : "";
  };
  let pushed = live ? await until("the harness's last push", async () => {
    const t = tip();
    return / push 2 /.test(` ${t.split(" ").slice(1, 3).join(" ")} `) ? t.split(" ")[0] : null;
  }, 6 * 60_000, 5000) : null;
  if (pushed) { await sleep(45_000); if (tip().split(" ")[0] !== pushed) { ev({ moved_after_last_push: tip() }); pushed = null; } }
  const heads = pushed ? g.run(["log", "--format=%H %s", "refs/remotes/origin/work"]).out.trim().split("\n").slice(0, 3).map((l) => l.split(" ")[0]) : [];
  ev({ work_head: pushed, heads });
  const pulls = (await api("GET", `${V3}/pulls?state=open&per_page=100`, admin)).body ?? [];
  const pr = Array.isArray(pulls) ? pulls.find((p) => p.head?.ref === work) : null;
  const node = pr ? await prNode(pr.number) : null;
  ev({ pr: pr?.number, draft: pr?.draft, user: pr?.user?.login, node_head: node?.headRefOid });
  if (!pr || !node) { gap("no draft pull request from the controller for the task's work branch"); verdict(); }
  // AgentMeta is the requester: the task names m3-admin.
  const cur = await until("the pull request's head at the second push", async () => ((await prNode(pr.number))?.headRefOid === pushed) && pushed, 2 * 60_000, 3000);
  check(!!cur, "the pull request's head is the harness's second push");
  // ---- (a), (b): the current head's run ----
  const runFor = async (head) => {
    for (const r of await runsOf(pr.number)) {
      const v = await runView(r.id);
      if (v.jobs.some((j) => j.name === "secret" && j.waits_on === head)) {
        // The shard's list carries no time; the run's GHES read has its `created_at`.
        return { ...v, created_at: (await api("GET", `${V3}/actions/runs/${r.id}`, admin)).body?.created_at };
      }
    }
    return null;
  };
  const run = await until("the run waiting on the current head", () => runFor(pushed), 4 * 60_000, 4000);
  ev({ run });
  if (!run) { check(false, "a run whose `secret` job waits on the current head"); verdict(); }
  check(run.fork === true, "the agent run is marked fork");
  for (const jn of ["plain", "missed"]) {
    const j = await finished(run.id, jn, 10 * 60_000);
    const lg = j ? await jobLog(j.id, jn) : null;
    const lag = j?.started_at ? Date.parse(j.started_at) - Date.parse(run.created_at) : null;
    ev({ job: jn, started_at: j?.started_at, run_to_start_ms: lag, conclusion: j?.conclusion, log: lg });
    check(lag !== null && lag <= 30_000, `\`${jn}\` starts within 30 s of its run (${lag} ms)`);
    if (jn === "missed") check(lg?.empty && lg.len === 0 && !lg.raw_value_in_log, "`missed` (a reference the scanner misses) runs with an empty secret");
    if (jn === "plain") check(!!lg && lg.token_write !== null && lg.token_write !== 201, `\`plain\`'s token cannot write (${lg?.token_write})`);
  }
  const waiting = async () => (await runView(run.id)).jobs.find((j) => j.name === "secret");
  const s0 = await waiting();
  ev({ secret_job: s0 });
  check(s0?.state === "SecretsWait" && s0.waits_on === pushed && s0.status === "waiting" && s0.steps === 0, "`secret` waits on its own head (SecretsWait, GHES waiting, no step run)");
  // ---- (s): a second agent-prefix pull request at the same commit, created before any approval ----
  const repoId = (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { id } }`)).body?.data?.repository?.id;
  const same = `graftyard/l004-same-${Date.now().toString(36)}/work`;
  ev({ same_push: g.run(["push", "-q", "origin", `${pushed}:refs/heads/${same}`]).ok, same_branch: same });
  const sp = await gql(admin, "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    { i: { repositoryId: repoId, baseRefName: "main", headRefName: same, title: "l004 same sha", body: "l004 same sha" } });
  const sn = sp.body?.data?.createPullRequest?.pullRequest?.number;
  ev({ same_pr: sn, errors: sp.body?.errors?.[0]?.message });
  const srun = sn ? await until("the same-SHA run waiting on the current head", async () => {
    for (const r of await runsOf(sn)) {
      const v = await runView(r.id);
      if (v.jobs.some((j) => j.name === "secret" && j.waits_on === pushed)) return v;
    }
    return null;
  }, 4 * 60_000, 4000) : null;
  ev({ same_run: srun });
  const sameSecret = async () => (srun ? (await runView(srun.id)).jobs.find((j) => j.name === "secret") : null);
  check(srun?.fork === true, "the same-SHA pull request's run is an agent run (fork) whose `secret` waits on the agent's head");
  // ---- (c): approvals that must release nothing ----
  const neg = async (what, tok, commit) => {
    const r = await approve(tok, node.id, commit);
    ev({ negative: what, status: r.status, errors: r.body?.errors?.[0]?.message, review: r.body?.data?.addPullRequestReview?.pullRequestReview?.state });
    await sleep(60_000);
    const s = await waiting();
    ev({ after: what, secret_job: s });
    check(s?.state === "SecretsWait" && s.status === "waiting" && s.steps === 0, `${what} releases nothing for 60 s`);
  };
  await neg("the requester's approval (m3-admin)", admin, pushed);
  const older = heads[1];
  if (older) await neg("m3-member's approval of an older head", member, older);
  else gap("no older head to approve");
  // ---- (d): the gate ----
  const still = (await prNode(pr.number))?.headRefOid;
  ev({ head_before_approval: still });
  check(still === pushed, "the pull request's head has not moved since the run (the approval names the current head)");
  const tApprove = Date.now();
  const ok = await approve(member, node.id, pushed);
  ev({ approve: ok.status, errors: ok.body?.errors?.[0]?.message, review: ok.body?.data?.addPullRequestReview?.pullRequestReview?.state });
  const rel = await until("`secret` released", async () => { const s = await waiting(); return s && s.state !== "SecretsWait" ? s : null; }, 5 * 60_000, 3000);
  const tRelease = Date.now();
  ev({ released: rel, approve_to_release_ms: rel ? tRelease - tApprove : null });
  check(!!rel, "m3-member's approval of the current head releases `secret`");
  const sj = rel ? await finished(run.id, "secret", 10 * 60_000) : null;
  const sl = sj ? await jobLog(sj.id, "secret") : null;
  ev({ secret_done: sj, log: sl });
  check(sl?.real && sl.len === value().length && !sl.raw_value_in_log, "the released `secret` reads the real secret (its hash and length; the value never printed)");
  check(!!sl && sl.token_write !== null && sl.token_write !== 201, `the released job's token write is still refused (${sl?.token_write})`);
  check(sj?.started_at && Date.parse(sj.started_at) >= tApprove - 2000, "the released job started after the approval");
  // ---- (s): the gate held for the agent's pull request only ----
  await sleep(60_000);
  const ss = await sameSecret();
  ev({ same_after_release: ss });
  check(ss?.state === "SecretsWait" && ss.waits_on === pushed && ss.steps === 0,
    "the same-SHA pull request's `secret` stays waiting 60 s after the agent's pull request's release (F150)");
  // ---- the fail line over every run of the pull request ----
  const all = [];
  for (const r of await runsOf(pr.number)) all.push(await runView(r.id));
  if (srun) all.push(await runView(srun.id));
  let leaks = 0, read = 0;
  for (const v of all) {
    for (const j of v.jobs) {
      if (!done(j) || (j.id === sj?.id)) continue;
      const lg = await jobLog(j.id, `all-${v.id}-${j.name}`);
      if (lg.real || lg.raw_value_in_log) { leaks++; ev({ leak: { run: v.id, job: j.name, waits_on: j.waits_on } }); }
      if (lg.hash) read++;
    }
  }
  const others = all.filter((v) => v.id !== run.id).flatMap((v) => v.jobs.filter((j) => j.name === "secret"));
  ev({ runs: all.length, other_secret_jobs: others.map((j) => ({ state: j.state, waits_on: j.waits_on, steps: j.steps })), secret_lines_read: read, leaks });
  check(all.every((v) => v.fork === true), "every agent run of both pull requests is fork");
  check(others.every((j) => j.state === "SecretsWait" && j.steps === 0), "every other head's `secret` stays waiting");
  check(leaks === 0, `no job of an agent run read the secret before AgentGate held for its head (${leaks} leaks)`);
  for (const v of all) ev({ cancel_run: v.id, status: (await api("POST", `${V3}/actions/runs/${v.id}/cancel`, admin)).status });
  // ---- control: a human's push ----
  const br = `human-${Date.now().toString(36)}`;
  g.run(["checkout", "-q", "-B", br, "origin/main"]);
  writeFileSync(join(g.wt, "human.txt"), `${br}\n`);
  g.run(["add", "-A"]); g.run(["commit", "-q", "-m", "l004 control"]);
  ev({ control_push: g.run(["push", "-q", "origin", `HEAD:refs/heads/${br}`]).ok });
  const cp = await gql(admin, "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    { i: { repositoryId: repoId, baseRefName: "main", headRefName: br, title: "l004 control", body: "l004 control" } });
  const cn = cp.body?.data?.createPullRequest?.pullRequest?.number;
  ev({ control_pr: cn, errors: cp.body?.errors?.[0]?.message });
  const crun = cn ? await until("the control run", async () => (await runsOf(cn))[0], 3 * 60_000, 3000) : null;
  const cv = crun ? await runView(crun.id) : null;
  ev({ control_run: cv });
  check(cv?.fork === false && cv.jobs.every((j) => j.state !== "SecretsWait"), "control: a human's run is not fork and no job waits");
  const cj = crun ? await finished(crun.id, "secret", 10 * 60_000) : null;
  const cl = cj ? await jobLog(cj.id, "control") : null;
  ev({ control_secret: cj, log: cl });
  check(cl?.real && !cl.raw_value_in_log, "control: the human run's `secret` reads the secret at once");
  if (crun) ev({ cancel_control: (await api("POST", `${V3}/actions/runs/${crun.id}/cancel`, admin)).status });
  // ---- end the task ----
  ev({ unhold: g.run(["push", "-q", "origin", `:refs/heads/hold/${id}`]).ok });
  ev({ cancel_task: (await api("POST", `${V1}/tasks/${encodeURIComponent(id)}/cancel`, admin)).status });
  verdict();
} else if (line === "teardown") {
  ev({ delete: R, status: (await api("DELETE", V3, admin)).status });
  ev({ policy: (await api("PUT", `${V1}/agents/policy`, admin, { ci: "approve" })).status });
  if (existsSync(VALUE_FILE)) { spawnSync("shred", ["-u", VALUE_FILE]); ev({ value_file: "shredded" }); }
} else {
  console.error("line: repo l004 teardown");
  process.exit(2);
}
