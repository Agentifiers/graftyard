// LM-3's batched row on graft-m4a (`tests/am3/results/lm3-batch.md`): A59, the demoted create, assignment and the
// controller's create, `agent-ci`, and F136's `/pulls` side check, all through graft-m4a-edge. The agents half (Apps,
// the controller with its `TaskBox`, `graft-agents-front`) is up first (`tests/am3/hosted/batch-row.sh`).
//
//   node tests/am3/hosted/batch.mjs <line> <out dir> <repo name> [ctrl token file] [controller url]
//
// Lines: repo | a59 | demote | assign | agentci | pulls. Each appends JSON rows to <out>/<line>.jsonl and prints one
// `VERDICT <line> PASS|FAIL <why>` line. No token is printed (`lib.mjs`'s `scan`).
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, ghs, log, secret, sleep, state, tokens } from "../../am1/hosted/lib.mjs";

const [line, out, name, tokenFile, ctrl] = process.argv.slice(2);
mkdirSync(out, { recursive: true });
const R = `m3org/${name}`, V3 = `/api/v3/repos/${R}`, V1 = `/api/graftyard/v1/repos/${R}`;
const admin = tokens.admin, member = tokens.member;
const AGENT_APP = 3801088, BOT = -AGENT_APP, BOT_LOGIN = "graftyard-agent[bot]";
const rows = join(out, `${line}.jsonl`);
const ev = (v) => { appendFileSync(rows, JSON.stringify({ t: Date.now(), ...v }) + "\n"); log(JSON.stringify(v)); };
const fails = [];
const check = (ok, what) => { ev({ check: what, ok }); if (!ok) fails.push(what); };
const verdict = () => { console.log(`VERDICT ${line} ${fails.length ? "FAIL" : "PASS"} ${fails.join("; ")}`); process.exit(fails.length ? 1 : 0); };
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
let ctoken = null;
const box = async (task) => {
  ctoken ??= secret(readFileSync(tokenFile, "utf8").trim());
  const r = await fetch(`${ctrl}/_controller/box/${task}/status`, { headers: { "x-controller-token": ctoken }, signal: AbortSignal.timeout(60_000) });
  return r.json().catch(() => null);
};
// A task read that does not answer 200 is retried (r1's A59 lost its last read to one such answer).
const task = async (id) => {
  for (let i = 0; ; i++) {
    const r = await api("GET", `${V1}/tasks/${encodeURIComponent(id)}`, admin);
    if (r.status === 200 || i >= 4) { if (r.status !== 200) ev({ task_read: id, status: r.status }); return r.status === 200 ? r.body : null; }
    await sleep(2000);
  }
};
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
const pushBranch = (g, br, file, text, from = "origin/main") => {
  g.run(["fetch", "-q", "origin"]);
  g.run(["checkout", "-q", "-B", br, from]);
  mkdirSync(join(g.wt, file, ".."), { recursive: true });
  writeFileSync(join(g.wt, file), text);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `${br} ${file}`]);
  const p = g.run(["push", "-q", "-f", "origin", `HEAD:refs/heads/${br}`]);
  return p.ok ? g.run(["rev-parse", "HEAD"]).out.trim() : (ev({ push: br, error: p.out }), null);
};
const repoNode = async () => (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { id } }`)).body?.data?.repository?.id;
const openPr = async (head, title) => {
  const r = await gql(admin, "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number headRefName } } }",
    { i: { repositoryId: await repoNode(), baseRefName: "main", headRefName: head, title, body: title } });
  ev({ pr: head, status: r.status, number: r.body?.data?.createPullRequest?.pullRequest?.number, errors: r.body?.errors?.[0]?.message });
  return r.body?.data?.createPullRequest?.pullRequest;
};
const newIssue = async (title) => {
  const r = await api("POST", `${V3}/issues`, admin, { title });
  ev({ issue: title, status: r.status, number: r.body?.number });
  return r.body;
};
// `GET /issues/{n}` is not served on main (404 for every number, r1); the list is.
const assignees = async (n) => (((await api("GET", `${V3}/issues?state=all&per_page=100`, admin)).body ?? []).find((x) => x.number === n)?.assignees ?? []).map((a) => a.login);
const tasksOf = async (issue) => {
  const r = await api("GET", `${V1}/tasks?issue_number=${issue}&per_page=100`, admin);
  return Array.isArray(r.body) ? r.body : [];
};

if (line === "repo") {
  // The row's repository, with a workflow every pull request runs, so the agent-ci lines have a run to hold.
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: true, auto_init: true });
  ev({ create: R, status: made.status });
  if (made.status !== 201) process.exit(1);
  const g = await clone();
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, ".github/workflows/ci.yml"), "on: pull_request\njobs:\n  a:\n    runs-on: self-hosted\n    steps:\n      - run: true\n  b:\n    needs: a\n    runs-on: self-hosted\n    steps:\n      - run: true\n");
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", "ci"]);
  ev({ push_main: g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok });
} else if (line === "a59") {
  // A59: the TaskBox moves the base's task; nothing here patches it. Task 1 runs to `Finished`; task 2 is cancelled
  // by the admin, and the box must not move it back.
  const mk = async (k) => {
    const r = await api("POST", `${V1}/tasks`, admin, { prompt: `LM-3 batch A59 ${k}`, summary: `a59 ${k}` });
    ev({ create: k, status: r.status, id: r.body?.id, state: r.body?.state });
    return r.status === 201 ? r.body.id : null;
  };
  const t1 = await mk("run"), t2 = await mk("cancel");
  check(!!t1 && !!t2, "both creates 201");
  if (!t1 || !t2) verdict();
  const running = await until("t1 in_progress", async () => (await task(t1))?.state === "in_progress" && (await task(t1)), 6 * 60_000);
  ev({ t1_running: running?.state, box: (await box(t1))?.box?.task?.state });
  check(!!running, "the base reads in_progress while the box runs the task, with no PATCH by the row");
  await until("t2 running", async () => (await box(t2))?.box?.task?.state?.Running, 6 * 60_000);
  const c = await api("POST", `${V1}/tasks/${encodeURIComponent(t2)}/cancel`, admin);
  ev({ cancel: t2, status: c.status, state: c.body?.state });
  check(c.status === 200 && c.body?.state === "cancelled", "the admin's cancel answers 200 cancelled");
  const done = await until("t1 completed", async () => (await task(t1))?.state === "completed" && (await task(t1)), 10 * 60_000, 10_000);
  ev({ t1_end: done?.state, box: (await box(t1))?.box?.task?.state, base_seen: (await box(t1))?.box?.base });
  check(!!done, "the base reads completed after the harness's Finished");
  await sleep(90_000); // past two `HEARTBEAT` alarms of the cancelled box
  const after = await task(t2);
  ev({ t2_after: after?.state, box: (await box(t2))?.box?.task?.state });
  check(after?.state === "cancelled", "a cancelled task stays cancelled: the box sends no state over it");
} else if (line === "demote") {
  // 60 committed writes by m3-admin inside DEMOTE_WINDOW (one minute), then a Graftyard-run create in the same
  // window: NotWriter's 403. A control create after the window passes is 201.
  // The writes are GraphQL `addComment`s (`POST .../issues/{n}/comments` is not served on main: 404, r1).
  const iss = await newIssue("demote target");
  const node = (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { issue(number: ${iss.number}) { id } } }`)).body?.data?.repository?.issue?.id;
  const t0 = Date.now();
  // 62 committed writes (two over the line), 4 at a time; a 503 is retried, since r1's 8-way burst lost 15 of 64 to
  // 503 and committed only 49.
  let ok = 0, failed = 0, first = null, next = 0;
  await Promise.all(Array.from({ length: 4 }, async () => {
    while (ok < 62 && Date.now() - t0 < 45_000) {
      const i = next++;
      const r = await gql(admin, "mutation($i: AddCommentInput!) { addComment(input: $i) { commentEdge { node { id } } } }", { i: { subjectId: node, body: `w${i}` } });
      if (r.status === 200 && !r.body?.errors) ok += 1;
      else { failed += 1; first ??= { status: r.status, errors: r.body?.errors?.[0]?.message }; await sleep(300); }
    }
  }));
  ev({ failed_writes: failed, first_failed_write: first });
  const spent = Date.now() - t0;
  ev({ writes: ok, ms: spent });
  check(ok >= 60 && spent < 50_000, `60 writes inside the window (${ok} in ${spent} ms)`);
  const r = await api("POST", `${V1}/tasks`, admin, { prompt: "LM-3 batch demoted create", summary: "demote" });
  ev({ demoted_create: r.status, message: r.body?.message, id: r.body?.id });
  check(r.status === 403, "the create inside the window is 403 (NotWriter)");
  if (r.status === 201) await api("POST", `${V1}/tasks/${encodeURIComponent(r.body.id)}/cancel`, admin);
  await sleep(Math.max(0, 75_000 - (Date.now() - t0)));
  const c = await api("POST", `${V1}/tasks`, admin, { prompt: "LM-3 batch demote control", summary: "demote control" });
  ev({ control_create: c.status, id: c.body?.id });
  check(c.status === 201, "the control create after the window is 201");
  if (c.status === 201) await api("POST", `${V1}/tasks/${encodeURIComponent(c.body.id)}/cancel`, admin);
} else if (line === "assign") {
  const me = (await api("GET", "/api/v3/user", admin)).body?.login;
  const created = async (n, what) => {
    const t0 = Date.now();
    const t = await until(`task from #${n}`, async () => (await tasksOf(n)).find((x) => x.kind === "graftyard_run"), 8 * 60_000);
    ev({ create_after_ms: Date.now() - t0 });
    ev({ [what]: n, task: t?.id, requester: t?.requester?.login, creator: t?.creator?.login, state: t?.state });
    return t;
  };
  // REST add by the writer.
  const a = await newIssue("assign rest");
  const ra = await api("POST", `${V3}/issues/${a.number}/assignees`, admin, { assignees: [BOT_LOGIN] });
  ev({ rest_add: ra.status, assignees: (ra.body?.assignees ?? []).map((x) => x.login) });
  check(ra.status === 201 && (ra.body?.assignees ?? []).some((x) => x.login === BOT_LOGIN), "REST add answers 201 with the bot assigned");
  const ta = await created(a.number, "rest_create");
  check(!!ta && ta.requester?.login === me, `the controller's create from the REST assignment names the assigner (${me}) as requester`);
  // GraphQL add, with the bot's BOT_ node id read from the REST issue.
  const b = await newIssue("assign graphql");
  const botNode = ra.body?.assignees?.find((x) => x.login === BOT_LOGIN)?.node_id;
  const node = (await gql(admin, `query { repository(owner: "m3org", name: "${name}") { issue(number: ${b.number}) { id } } }`)).body?.data?.repository?.issue?.id;
  const ga = await gql(admin, "mutation($i: AddAssigneesToAssignableInput!) { addAssigneesToAssignable(input: $i) { assignable { ... on Issue { number } } } }",
    { i: { assignableId: node, assigneeIds: [botNode] } });
  ev({ graphql_add: ga.status, errors: ga.body?.errors?.[0]?.message, assignees: await assignees(b.number) });
  check(ga.status === 200 && !ga.body?.errors && (await assignees(b.number)).includes(BOT_LOGIN), "GraphQL add assigns the bot");
  const tb = await created(b.number, "graphql_create");
  check(!!tb && tb.requester?.login === me, "the controller's create from the GraphQL assignment names the assigner");
  // DELETE, then an assignees-only PATCH that re-adds: the PATCH counts as an assignment.
  const d = await api("DELETE", `${V3}/issues/${a.number}/assignees`, admin, { assignees: [BOT_LOGIN] });
  ev({ rest_remove: d.status, assignees: (d.body?.assignees ?? []).map((x) => x.login) });
  check(d.status === 200 && !(d.body?.assignees ?? []).some((x) => x.login === BOT_LOGIN), "REST remove answers 200 without the bot");
  const p = await api("PATCH", `${V3}/issues/${a.number}`, admin, { assignees: [BOT_LOGIN, me] });
  ev({ patch: p.status, assignees: (p.body?.assignees ?? []).map((x) => x.login) });
  check(p.status === 200 && (p.body?.assignees ?? []).map((x) => x.login).includes(BOT_LOGIN), "an assignees-only PATCH answers 200 and sets the bot");
  // A second writer's assignment (m3-member has write on the row's repository, r1): the task's requester is that writer.
  const perm = (await api("GET", `${V3}/collaborators/m3-member/permission`, admin)).body?.permission;
  const c = await newIssue("assign by second writer");
  const rm = await api("POST", `${V3}/issues/${c.number}/assignees`, member, { assignees: [BOT_LOGIN] });
  ev({ member_permission: perm, member_add: rm.status, assignees: await assignees(c.number) });
  const tm = await created(c.number, "member_create");
  check(!!tm && tm.requester?.id === 7100002, `a second writer's assignment creates with that writer as requester (id ${tm?.requester?.id}, login ${tm?.requester?.login})`);
  // Another App's token cannot create a Graftyard-run task.
  const ci = (await ghs({})).body?.token;
  const rc = await api("POST", `${V1}/tasks`, ci, { issue_number: c.number });
  ev({ other_app_create: rc.status, message: rc.body?.message });
  check(rc.status === 403, "another App's create is 403");
  for (const t of [ta, tb, tm]) if (t) await api("POST", `${V1}/tasks/${encodeURIComponent(t.id)}/cancel`, admin);
} else if (line === "agentci") {
  const g = await clone();
  const ci = (await ghs({})).body?.token;
  // A pull request run is keyed by its test merge: `head_ref` is `refs/pull/<n>/merge`, `head_sha` the merge commit.
  const runOf = async (pr) => {
    const r = await api("GET", `${V1}/actions/runs`, admin);
    return (r.body?.runs ?? []).find((x) => x.event === "pull_request" && x.head_ref === `refs/pull/${pr}/merge`);
  };
  const shape = async (id) => {
    const r = await api("GET", `${V3}/actions/runs/${id}`, admin);
    const j = await api("GET", `${V3}/actions/runs/${id}/jobs`, admin);
    const row = await api("GET", `${V1}/actions/runs/${id}`, admin);
    return { status: r.body?.status, conclusion: r.body?.conclusion, jobs: j.body?.total_count, fork: row.body?.value?.run?.run?.fork,
      states: (row.body?.value?.jobs ?? []).map((x) => Object.keys(x.state ?? {})[0]) };
  };
  // Held: a same-repository pull request from m3-ci's confine, under the default `ci: approve`.
  const pol = await api("GET", `${V1}/agents/policy`, admin);
  ev({ policy: pol.status, ci: pol.body?.ci });
  pushBranch(g, `claude/held-${Date.now() % 1e6}`, "held.txt", "held\n");
  const br1 = g.run(["rev-parse", "--abbrev-ref", "HEAD"]).out.trim();
  const pr1 = await openPr(br1, "agent-ci held");
  const run1 = await until("held run", () => runOf(pr1?.number), 3 * 60_000);
  ev({ held_run: run1 });
  if (!run1) { check(false, "a run for the confined pull request"); verdict(); }
  await sleep(20_000);
  const s1 = await shape(run1.id);
  ev({ held_shape: s1 });
  check(s1.status === "completed" && s1.conclusion === "action_required", "a held run reads completed / action_required");
  check(s1.jobs === 0, "a held run lists no jobs");
  check(s1.states.every((x) => x === "Held"), "every job of the run is Held in the shard");
  const checks = await api("GET", `${V3}/commits/${run1.head_sha}/check-runs`, admin);
  ev({ held_checks: checks.status, total: checks.body?.total_count });
  check(!(checks.body?.total_count > 0), "a held run has no check run on its head");
  // An Agent's token is refused; a writer approves; a second approval is 409.
  const ap = (tok) => api("POST", `${V1}/actions/runs/${run1.id}/approve`, tok);
  const a1 = await ap(ci);
  ev({ approve_agent: a1.status, message: a1.body?.message });
  check(a1.status === 403 && a1.body?.message === "Resource not accessible by integration", "the Agent's token gets 403 Resource not accessible by integration");
  const a2 = await ap(admin);
  ev({ approve_writer: a2.status, body: a2.body });
  check(a2.status === 201, "the writer's approval answers 201");
  const s2 = await until("approved run queued", async () => { const s = await shape(run1.id); return s.status === "queued" && s; }, 60_000, 3000);
  ev({ approved_shape: s2 });
  check(!!s2 && s2.jobs === 2 && s2.states.join() === "Queued,Pending", "after approval the run reads queued, its first job Queued and its dependent Pending");
  check(s2?.fork === true, "the approved run stays fork");
  const a3 = await ap(admin);
  ev({ approve_again: a3.status, message: a3.body?.message });
  check(a3.status === 409, "a second approval is 409");
  // AutoReadOnly: queued at once, still fork.
  const put = await api("PUT", `${V1}/agents/policy`, admin, { ci: "auto_read_only" });
  ev({ policy_put: put.status, ci: put.body?.ci });
  pushBranch(g, `claude/auto-${Date.now() % 1e6}`, "auto.txt", "auto\n");
  const br2 = g.run(["rev-parse", "--abbrev-ref", "HEAD"]).out.trim();
  const pr2 = await openPr(br2, "agent-ci auto");
  const run2 = await until("auto run", () => runOf(pr2?.number), 3 * 60_000);
  const s3 = run2 ? await shape(run2.id) : null;
  ev({ auto_run: run2, auto_shape: s3 });
  check(s3?.status === "queued" && s3.jobs === 2, "under AutoReadOnly the run reads queued at once with its jobs listed");
  check(s3?.fork === true, "the AutoReadOnly run is fork");
  const back = await api("PUT", `${V1}/agents/policy`, admin, { ci: "approve" });
  ev({ policy_back: back.status, ci: back.body?.ci });
  // Cancel both runs so nothing waits on a runner.
  for (const r of [run1, run2]) if (r) ev({ cancel: r.id, status: (await api("POST", `${V3}/actions/runs/${r.id}/cancel`, admin)).status });
} else if (line === "auto") {
  // agent-ci's AutoReadOnly line alone (r1's GHES run read answered 503 once): every read try is recorded with its
  // status and message, up to 3 tries 5 s apart.
  const g = await clone();
  const runOf = async (pr) => ((await api("GET", `${V1}/actions/runs`, admin)).body?.runs ?? []).find((x) => x.head_ref === `refs/pull/${pr}/merge`);
  const put = await api("PUT", `${V1}/agents/policy`, admin, { ci: "auto_read_only" });
  ev({ policy_put: put.status, ci: put.body?.ci });
  pushBranch(g, `claude/auto2-${Date.now() % 1e6}`, "auto2.txt", "auto2\n");
  const pr = await openPr(g.run(["rev-parse", "--abbrev-ref", "HEAD"]).out.trim(), "agent-ci auto 2");
  const run = await until("auto run", () => runOf(pr?.number), 3 * 60_000);
  ev({ auto_run: run });
  let r = null;
  for (let i = 0; i < 3; i++) {
    r = await api("GET", `${V3}/actions/runs/${run?.id}`, admin);
    ev({ read: i, status: r.status, run_status: r.body?.status, conclusion: r.body?.conclusion, message: r.status === 200 ? undefined : r.body?.message });
    if (r.status === 200) break;
    await sleep(5000);
  }
  const j = await api("GET", `${V3}/actions/runs/${run?.id}/jobs`, admin);
  const row = await api("GET", `${V1}/actions/runs/${run?.id}`, admin);
  const states = (row.body?.value?.jobs ?? []).map((x) => Object.keys(x.state ?? {})[0]);
  ev({ jobs: j.status, total: j.body?.total_count, fork: row.body?.value?.run?.run?.fork, states });
  check(r?.status === 200 && r.body?.status === "queued" && j.body?.total_count === 2, "under AutoReadOnly the run reads queued at once with its jobs listed");
  check(row.body?.value?.run?.run?.fork === true && !states.includes("Held"), "the AutoReadOnly run is fork and never held");
  ev({ policy_back: (await api("PUT", `${V1}/agents/policy`, admin, { ci: "approve" })).status });
  if (run) ev({ cancel: run.id, status: (await api("POST", `${V3}/actions/runs/${run.id}/cancel`, admin)).status });
} else if (line === "pulls") {
  // F136: one open and one closed (merged) pull request, then the list with state=all, a head filter and a base filter.
  const g = await clone();
  const t = Date.now() % 1e6;
  pushBranch(g, `f136-open-${t}`, "open.txt", "open\n");
  pushBranch(g, `f136-merge-${t}`, "merge.txt", "merge\n");
  const po = await openPr(`f136-open-${t}`, "f136 open");
  const pm = await openPr(`f136-merge-${t}`, "f136 merged");
  const m = await api("PUT", `${V3}/pulls/${pm?.number}/merge`, admin, { merge_method: "merge" });
  ev({ merge: m.status, merged: m.body?.merged });
  const REQ = ["url", "id", "node_id", "html_url", "diff_url", "patch_url", "issue_url", "commits_url", "review_comments_url",
    "review_comment_url", "comments_url", "statuses_url", "number", "state", "locked", "title", "user", "body", "labels",
    "milestone", "created_at", "updated_at", "closed_at", "merged_at", "merge_commit_sha", "assignee", "assignees",
    "requested_reviewers", "requested_teams", "head", "base", "_links", "author_association", "auto_merge"];
  const list = async (q) => {
    const r = await api("GET", `${V3}/pulls?${q}`, admin);
    const items = Array.isArray(r.body) ? r.body : [];
    const missing = [...new Set(items.flatMap((x) => REQ.filter((k) => !(k in x))))];
    ev({ list: q, status: r.status, numbers: items.map((x) => [x.number, x.state]), missing });
    return { status: r.status, items, missing };
  };
  const all = await list("state=all");
  check(all.status === 200 && all.missing.length === 0, "state=all answers 200 with pull-request-simple's required fields");
  check(all.items.some((x) => x.number === po?.number && x.state === "open") && all.items.some((x) => x.number === pm?.number && x.state === "closed"),
    "state=all lists the open and the closed pull request");
  const head = await list(`state=all&head=m3org:f136-open-${t}`);
  check(head.status === 200 && head.items.length === 1 && head.items[0].number === po?.number && head.missing.length === 0, "head=owner:branch lists only its pull request");
  const base = await list("state=closed&base=main");
  check(base.status === 200 && base.items.some((x) => x.number === pm?.number) && base.items.every((x) => x.state === "closed") && base.missing.length === 0,
    "base=main with state=closed lists the merged pull request only among closed");
} else if (line === "delete") {
  ev({ delete: R, status: (await api("DELETE", V3, admin)).status });
} else {
  console.error("line: repo a59 demote assign agentci pulls delete");
  process.exit(2);
}
if (line !== "repo" && line !== "delete") verdict();
