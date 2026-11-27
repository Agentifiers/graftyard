// F137's hosted row on graft-m4a (`tests/am3/results/f137-row.md`): F137's issue writes (32.35), label update (32.36)
// and milestones (32.37), each read back over REST and GraphQL, driven by gh 2.102.1 and Octokit through graft-m4a-edge
// with m3-admin's PAT and m3-ci's installation tokens. `f137-row.sh` runs the days around these lines.
//
//   node tests/am3/hosted/f137.mjs <line> <out dir> <repo name>
//
// Lines, in order:
//   repo     m3org/<repo name> with branch `f` and pull request A, issue I with one comment, and the label `plain`;
//            m3-ci's installation permissions read (the App lines need issues and pull_requests write)
//   base     before any day: every request below answers as main does with 32.35 to 32.37 off, and changes nothing
//   octokit  after the edge day: Octokit's issue PATCH, comment edit, lock and unlock, label read, update and delete,
//            the six milestone routes, an issue's milestone by PATCH and create, and m3-ci's milestone writes
//   gh       after the edge day: gh's label create, edit and delete, `issue edit -t -b`, `-m`, `--remove-milestone`,
//            `-t -b -m`, `issue create -m`, `pr edit -m` and `--remove-milestone`, `issue view`; gh's own IssueUpdate
//            document with an unknown milestoneId (NOT_FOUND, nothing changed); and the gap probes (`gh issue lock`,
//            `gh issue comment --edit-last`, `gh issue list -m`), which must change nothing
//   delete   the repository deleted
// F137_LOCAL=1 is the $0 dry run on a local bed (`tests/plaindiff`'s builds in workerd, F137_OWNER its user): the
// repository is a user's, and the lines that need a git shard or graft-ops (branch f, pull request A, m3-ci's tokens)
// are skipped and logged as skipped.
// Each line appends JSON rows to <out>/<line>.jsonl and prints one `VERDICT <line> PASS|FAIL <why>` line. No token is
// printed or put on argv (`lib.mjs`'s `scan`; gh reads its token from the environment).
import { execFileSync } from "node:child_process";
import { appendFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Octokit } from "../../m0/wp15/node_modules/@octokit/rest/dist-src/index.js";
import { createAppAuth } from "../../m0/wp15/node_modules/@octokit/auth-app/dist-node/index.js";
import { EDGE, HOST, Git, api, log, scan, secret, sleep, state, tokens } from "../../am1/hosted/lib.mjs";

const [line, out, name] = process.argv.slice(2);
if (!line || !out || !name) { console.error("usage: f137.mjs <line> <out dir> <repo name>"); process.exit(2); }
mkdirSync(out, { recursive: true });
const OWNER = process.env.F137_OWNER ?? "m3org", LOCAL = process.env.F137_LOCAL === "1", R = `${OWNER}/${name}`, V3 = `/api/v3/repos/${R}`;
const admin = process.env.F137_TOKEN_FILE ? secret(readFileSync(process.env.F137_TOKEN_FILE, "utf8").trim()) : tokens.admin;
const ROOT = new URL("../../..", import.meta.url).pathname;
const BOT_LOGIN = "m3-ci[bot]";
const rows = join(out, `${line}.jsonl`);
const ev = (v) => { appendFileSync(rows, scan(JSON.stringify({ t: Date.now(), ...v })) + "\n"); log(JSON.stringify(v)); };
const fails = [];
const check = (ok, what, seen) => { ev({ check: what, ok, ...(ok ? {} : { seen }) }); if (!ok) fails.push(what); return ok; };
const verdict = () => { console.log(`VERDICT ${line} ${fails.length ? "FAIL" : "PASS"} ${fails.join("; ")}`); process.exit(fails.length ? 1 : 0); };
const saved = join(out, "..", `f137-${name}.json`);
const S = () => JSON.parse(readFileSync(saved, "utf8"));

// ---- clients ----

const gql = async (query, variables, tok = admin) => (await api("POST", "/api/graphql", tok, { query, variables })).body;
const octokit = (auth) => new Octokit({ baseUrl: `${EDGE}/api/v3`, auth, userAgent: "graftyard-f137 octokit", log: { debug() {}, info() {}, warn() {}, error() {} } });
const call = async (o, route, params = {}) => {
  try { const r = await o.request(route, { owner: OWNER, repo: name, ...params }); return { status: r.status, data: r.data }; }
  catch (e) { return { status: e.status ?? 0, data: e.response?.data ?? String(e).slice(0, 200) }; }
};
// An m3-ci installation token, narrowed to `permissions` when given, minted by @octokit/auth-app through the edge.
const appToken = async (permissions) => {
  const o = new Octokit({ baseUrl: `${EDGE}/api/v3`, authStrategy: createAppAuth,
    auth: { appId: state.app, privateKey: readFileSync(`${process.env.M3_STATE}/app.pem`, "utf8"), installationId: state.installation } });
  const a = await o.auth({ type: "installation", ...(permissions ? { permissions } : {}) });
  return { token: secret(a.token), permissions: a.permissions };
};
// gh 2.102.1 against the edge as m3-admin, its token in the environment only, in a throwaway config directory.
const ghDir = mkdtempSync(join(tmpdir(), "f137-gh-"));
const gh = (...args) => {
  if (LOCAL) { ev({ gh: args.join(" "), skipped: "F137_LOCAL: gh needs TLS on a real host" }); return null; }
  const env = { ...process.env, HOME: ghDir, GH_CONFIG_DIR: ghDir, GH_HOST: HOST, GH_ENTERPRISE_TOKEN: admin,
    GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" };
  let code = 0, text;
  try { text = execFileSync("gh", [...args, "-R", `${HOST}/${R}`], { env, stdio: ["ignore", "pipe", "pipe"], timeout: 120_000 }).toString(); }
  catch (e) { code = e.status ?? 1; text = `${e.stdout ?? ""}${e.stderr ?? ""}`; }
  text = scan(text.replaceAll(`https://${HOST}`, "<edge>")).slice(0, 400);
  ev({ gh: args.join(" "), code, out: text });
  return { code, out: text };
};
process.on("exit", () => rmSync(ghDir, { recursive: true, force: true }));

// ---- reads, REST and GraphQL ----

const restIssue = async (n) => (await api("GET", `${V3}/issues/${n}`, admin)).body;
const gqlItem = async (n, pull = false) => (await gql(`query($o: String!, $r: String!, $n: Int!) { repository(owner: $o, name: $r) {
  item: ${pull ? "pullRequest" : "issue"}(number: $n) { id title body milestone { number title }
    comments(first: 50) { nodes { id body } } } } }`, { o: OWNER, r: name, n }))?.data?.repository?.item;
const restLabel = async (l) => api("GET", `${V3}/labels/${encodeURIComponent(l)}`, admin);
const gqlLabels = async () => ((await gql(`query($o: String!, $r: String!) { repository(owner: $o, name: $r) {
  labels(first: 100) { nodes { name color description } } } }`, { o: OWNER, r: name }))?.data?.repository?.labels?.nodes ?? []);
const gqlMilestone = async (m) => (await gql(`query($o: String!, $r: String!, $m: Int!) { repository(owner: $o, name: $r) {
  milestone(number: $m) { number title state description } } }`, { o: OWNER, r: name, m }))?.data?.repository?.milestone;
const gqlMilestones = async (states) => ((await gql(`query($o: String!, $r: String!, $s: [MilestoneState!]) { repository(owner: $o, name: $r) {
  milestones(states: $s, first: 100) { nodes { number title state } } } }`, { o: OWNER, r: name, s: states }))?.data?.repository?.milestones?.nodes);
const msOf = (it) => it?.milestone?.number ?? null;
// An item's state both surfaces show, for "changed nothing" checks.
const snapshot = async (n, pull = false) => {
  const r = await restIssue(n), g = await gqlItem(n, pull);
  return JSON.stringify({ title: r?.title, body: r?.body, ms: msOf(r), locked: r?.locked, updated: r?.updated_at,
    gtitle: g?.title, gbody: g?.body, gms: msOf(g), comments: g?.comments?.nodes?.map((c) => c.body) });
};
// gh's own documents (crates/api/gh), sent as gh sends them.
const ghDoc = (f) => JSON.parse(readFileSync(join(ROOT, "crates/api/gh", f), "utf8")).query;
const ghPost = (query, variables) => api("POST", "/api/graphql", admin, { query, variables }, { "user-agent": "GitHub CLI v2.102.1" });
const errType = (b) => b?.errors?.[0]?.type;
const errMsg = (b) => b?.errors?.[0]?.message ?? "";

if (line === "repo") {
  const made = await api("POST", LOCAL ? "/api/v3/user/repos" : `/api/v3/orgs/${OWNER}/repos`, admin, { name, private: true, auto_init: !LOCAL });
  check(made.status === 201, "the row's repository is created", made.status);
  if (made.status !== 201) verdict();
  const issue = await api("POST", `${V3}/issues`, admin, { title: "f137 I", body: "i" });
  const comment = await api("POST", `${V3}/issues/${issue.body?.number}/comments`, admin, { body: "first" });
  const label = await api("POST", `${V3}/labels`, admin, { name: "plain", color: "ededed" });
  const s = { issue: issue.body?.number, issueNode: issue.body?.node_id, comment: comment.body?.id };
  check(issue.status === 201 && comment.status === 201 && label.status === 201, "issue I, its comment and `plain` are made",
    { issue: issue.status, comment: comment.status, label: label.status });
  if (LOCAL) { ev({ skipped: "branch f, pull request A and m3-ci's permissions (F137_LOCAL)" }); writeFileSync(saved, JSON.stringify(s)); verdict(); }
  const g = new Git(R).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
  g.run(["checkout", "-q", "-b", "f"]);
  writeFileSync(join(g.wt, "f.txt"), "f137\n");
  g.run(["add", "-A"]); g.run(["commit", "-q", "-m", "f"]);
  check(g.run(["push", "-q", "origin", "HEAD:refs/heads/f"]).ok, "branch f pushed");
  const repoId = (await gql(`query { repository(owner: "${OWNER}", name: "${name}") { id } }`))?.data?.repository?.id;
  const pr = await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    { i: { repositoryId: repoId, baseRefName: "main", headRefName: "f", title: "f137 A", body: "a" } });
  s.pr = pr?.data?.createPullRequest?.pullRequest?.number;
  ev({ seeded: s });
  check(!!s.pr, "pull request A is open", pr);
  const t = await appToken();
  ev({ m3ci_permissions: t.permissions });
  check(t.permissions?.issues === "write" && t.permissions?.pull_requests === "write",
    "m3-ci's installation holds issues and pull_requests write (the App lines need both)", t.permissions);
  writeFileSync(saved, JSON.stringify(s));
} else if (line === "base") {
  // What main answers with 32.35, 32.36 and 32.37 off: `tests/plaindiff/run.sh`'s base column on main 897e5816 (the
  // edge code these lines reach is the same on 4fb7f48e), and the issue-writes, label-routes and milestones records.
  const s = S(), i = s.issue, notFound = (r) => r.status === 404 && r.body?.message === "Not Found";
  const before = await snapshot(i), prBefore = s.pr ? await snapshot(s.pr, true) : null, labelBefore = JSON.stringify((await restLabel("plain")).body);
  const rest = [
    ["issue PATCH title and body (32.35)", "PATCH", `${V3}/issues/${i}`, { title: "base", body: "base" }, notFound],
    ["comment PATCH (32.35)", "PATCH", `${V3}/issues/comments/${s.comment}`, { body: "base" }, notFound],
    ["issue lock (32.35)", "PUT", `${V3}/issues/${i}/lock`, {}, notFound],
    ["issue unlock (32.35)", "DELETE", `${V3}/issues/${i}/lock`, undefined, notFound],
    ["label PATCH (32.36)", "PATCH", `${V3}/labels/plain`, { color: "000000" }, notFound],
    ["label GET by name (no format)", "GET", `${V3}/labels/plain`, undefined, (r) => r.status === 200 && r.body?.name === "plain"],
    ["milestones GET (32.37)", "GET", `${V3}/milestones`, undefined, notFound],
    ["milestone POST (32.37)", "POST", `${V3}/milestones`, { title: "base" }, notFound],
    ["milestone 1 GET (32.37)", "GET", `${V3}/milestones/1`, undefined, notFound],
    ["issue PATCH milestone (32.37)", "PATCH", `${V3}/issues/${i}`, { milestone: 1 }, notFound],
  ];
  for (const [what, method, path, body, want] of rest) {
    const r = await api(method, path, admin, body);
    check(want(r), `base: ${what} answers as main`, { status: r.status, body: JSON.stringify(r.body).slice(0, 160) });
  }
  // An issue create naming `milestone` is refused by main's create (32.32's `plan_create_issue`): 422 `invalid`.
  const made = await api("POST", `${V3}/issues`, admin, { title: "base create", milestone: 1 });
  check(made.status === 422 && made.body?.errors?.[0]?.field === "milestone", "base: an issue create naming milestone is 422 invalid, as on main",
    { status: made.status, body: JSON.stringify(made.body).slice(0, 160) });
  const undef = (b, f) => errType(b) === "undefinedField" && errMsg(b).includes(`'${f}'`);
  const ms = await gql(`query { repository(owner: "${OWNER}", name: "${name}") { milestones(first: 5) { nodes { title } } } }`);
  check(undef(ms, "milestones"), "base: Repository.milestones is undefinedField", ms);
  const m1 = await gql(`query { repository(owner: "${OWNER}", name: "${name}") { milestone(number: 1) { title } } }`);
  check(undef(m1, "milestone"), "base: Repository.milestone is undefinedField", m1);
  const up = (await ghPost(ghDoc("IssueUpdate-1.json"), { input: { id: s.issueNode, title: "base" } })).body;
  check(errType(up) === "unsupported" && errMsg(up) === "a mutation runs through plan", "base: gh's IssueUpdate is routed nowhere, as on main", up);
  const repoId = (await gql(`query { repository(owner: "${OWNER}", name: "${name}") { id } }`))?.data?.repository?.id;
  const ci = (await ghPost(ghDoc("IssueCreate-1.json"), { input: { repositoryId: repoId, title: "base", milestoneId: s.issueNode } })).body;
  check(ci?.data?.createIssue === null && errMsg(ci) === "milestoneId is not served", "base: createIssue with milestoneId is not served", ci);
  if (s.pr) {
    const prNode = (await gqlItem(s.pr, true))?.id;
    const pu = (await ghPost(ghDoc("PullRequestUpdate-1.json"), { input: { pullRequestId: prNode, milestoneId: s.issueNode } })).body;
    check(errMsg(pu) === "milestoneId is not served", "base: updatePullRequest with milestoneId is not served", pu);
  }
  check((await gqlItem(i))?.milestone === null, "base: Issue.milestone is null");
  const le = gh("label", "edit", "plain", "-d", "base");
  if (le) check(le.code !== 0 && le.out.includes("HTTP 404"), "base: gh label edit fails on its 404", le);
  const ie = gh("issue", "edit", String(i), "-m", "base");
  if (ie) check(ie.code !== 0, "base: gh issue edit -m fails (no milestone list)", ie);
  check(await snapshot(i) === before && (!s.pr || await snapshot(s.pr, true) === prBefore), "base: issue I and pull request A are unchanged", await snapshot(i));
  check(JSON.stringify((await restLabel("plain")).body) === labelBefore, "base: the label `plain` is unchanged");
} else if (line === "octokit") {
  const s = S(), i = s.issue, o = octokit(admin);
  // 32.35: one PATCH sets title and body together.
  const up = await call(o, "PATCH /repos/{owner}/{repo}/issues/{issue_number}", { issue_number: i, title: "ok title", body: "ok body" });
  check(up.status === 200 && up.data?.title === "ok title" && up.data?.body === "ok body", "issues.update title and body: 200 with both", up.status);
  let g = await gqlItem(i), r = await restIssue(i);
  check(r?.title === "ok title" && r?.body === "ok body" && g?.title === "ok title" && g?.body === "ok body", "issue PATCH read back over REST and GraphQL", { r: r?.title, g: g?.title });
  const ce = await call(o, "PATCH /repos/{owner}/{repo}/issues/comments/{comment_id}", { comment_id: s.comment, body: "edited" });
  check(ce.status === 200 && ce.data?.body === "edited", "issues.updateComment: 200 with the new body", ce.status);
  const rc = (await api("GET", `${V3}/issues/${i}/comments`, admin)).body ?? [];
  g = await gqlItem(i);
  check(rc.find((c) => c.id === s.comment)?.body === "edited" && g?.comments?.nodes?.some((c) => c.body === "edited"), "comment edit read back over REST and GraphQL");
  const lk = await call(o, "PUT /repos/{owner}/{repo}/issues/{issue_number}/lock", { issue_number: i, lock_reason: "resolved" });
  check(lk.status === 204, "issues.lock: 204", lk.status);
  check((await restIssue(i))?.locked === true, "lock read back over REST (locked: true)");
  const lockG = await gql(`query { repository(owner: "${OWNER}", name: "${name}") { issue(number: ${i}) { locked } } }`);
  ev({ gap: "Issue.locked over GraphQL", answer: lockG });
  const ul = await call(o, "DELETE /repos/{owner}/{repo}/issues/{issue_number}/lock", { issue_number: i });
  check(ul.status === 204 && (await restIssue(i))?.locked === false, "issues.unlock: 204, read back locked: false", ul.status);
  // 32.36: one label by name, its update, and its delete.
  const gl = await call(o, "GET /repos/{owner}/{repo}/labels/{name}", { name: "PLAIN" });
  check(gl.status === 200 && gl.data?.name === "plain", "issues.getLabel matches case-insensitively", gl.status);
  const ul2 = await call(o, "PATCH /repos/{owner}/{repo}/labels/{name}", { name: "plain", new_name: "plain2", color: "112233", description: "d" });
  check(ul2.status === 200 && ul2.data?.name === "plain2" && ul2.data?.color === "112233", "issues.updateLabel: 200, renamed and recoloured", ul2.status);
  const lr = await restLabel("plain2"), lg = (await gqlLabels()).find((l) => l.name === "plain2");
  check(lr.status === 200 && lr.body?.description === "d" && lg?.color === "112233" && lg?.description === "d"
    && (await restLabel("plain")).status === 404, "label update read back over REST and GraphQL (old name 404)");
  // 32.37: the six milestone routes.
  const me = (await api("GET", "/api/v3/user", admin)).body?.login;
  const c1 = await call(o, "POST /repos/{owner}/{repo}/milestones", { title: "v1", description: "first", due_on: "2027-01-01T00:00:00Z" });
  const c2 = await call(o, "POST /repos/{owner}/{repo}/milestones", { title: "v2" });
  const m1 = c1.data?.number, m2 = c2.data?.number;
  check(c1.status === 201 && c2.status === 201 && c1.data?.creator?.login === me && !!me && m2 === m1 + 1, `issues.createMilestone twice: 201, numbered, creator ${me}`, [c1.status, c2.status, c1.data?.creator?.login]);
  const dup = await call(o, "POST /repos/{owner}/{repo}/milestones", { title: "V1" });
  check(dup.status === 422, "a second milestone titled V1 is 422", dup.status);
  const gm = await call(o, "GET /repos/{owner}/{repo}/milestones/{milestone_number}", { milestone_number: m1 });
  check(gm.status === 200 && gm.data?.title === "v1" && gm.data?.description === "first", "issues.getMilestone: 200", gm.status);
  const cl = await call(o, "PATCH /repos/{owner}/{repo}/milestones/{milestone_number}", { milestone_number: m2, state: "closed" });
  check(cl.status === 200 && cl.data?.state === "closed", "issues.updateMilestone closes v2: 200 closed", cl.status);
  const lo = await call(o, "GET /repos/{owner}/{repo}/milestones", {}), lc = await call(o, "GET /repos/{owner}/{repo}/milestones", { state: "closed" });
  check(lo.status === 200 && lo.data?.map((m) => m.title).join() === "v1" && lc.data?.map((m) => m.title).join() === "v2", "issues.listMilestones: open lists v1, closed lists v2", [lo.data?.length, lc.data?.length]);
  const gOpen = await gqlMilestones(["OPEN"]), gm2 = await gqlMilestone(m2);
  check(gOpen?.map((m) => m.title).join() === "v1" && gm2?.state === "CLOSED" && (await gqlMilestone(m1))?.description === "first", "milestones read back over GraphQL (open list, milestone(number:))", { gOpen, gm2 });
  // An issue's milestone: PATCH, clear, an unknown number, and a create that names one.
  const sm = await call(o, "PATCH /repos/{owner}/{repo}/issues/{issue_number}", { issue_number: i, milestone: m1 });
  check(sm.status === 200 && sm.data?.milestone?.number === m1, "issues.update milestone: 200 with v1", sm.status);
  check(msOf(await restIssue(i)) === m1 && msOf(await gqlItem(i)) === m1 && (await call(o, "GET /repos/{owner}/{repo}/milestones/{milestone_number}", { milestone_number: m1 })).data?.open_issues === 1,
    "issue milestone read back over REST and GraphQL, v1 counts 1 open issue");
  const bad = await call(o, "PATCH /repos/{owner}/{repo}/issues/{issue_number}", { issue_number: i, milestone: 999 });
  check(bad.status === 422 && msOf(await restIssue(i)) === m1, "an unknown milestone number is 422 and changes nothing", bad.status);
  const cm = await call(o, "PATCH /repos/{owner}/{repo}/issues/{issue_number}", { issue_number: i, milestone: null });
  check(cm.status === 200 && cm.data?.milestone === null && msOf(await gqlItem(i)) === null, "issues.update milestone null clears it (REST and GraphQL)", cm.status);
  const ci = await call(o, "POST /repos/{owner}/{repo}/issues", { title: "with v1", milestone: m1 });
  check(ci.status === 201 && ci.data?.milestone?.number === m1 && msOf(await gqlItem(ci.data?.number)) === m1, "issues.create with milestone: 201, read back over GraphQL", ci.status);
  const ll = await call(o, "GET /repos/{owner}/{repo}/milestones/{milestone_number}/labels", { milestone_number: m1 });
  check(ll.status === 200 && Array.isArray(ll.data), "issues.listLabelsForMilestone: 200", ll.status);
  if (LOCAL) ev({ skipped: "m3-ci's App lines (F137_LOCAL)" });
  else {
  // m3-ci: a pull_requests-only token writes milestones as its bot; a contents-only token is refused.
  const pr = octokit((await appToken({ pull_requests: "write", metadata: "read" })).token);
  const ac = await call(pr, "POST /repos/{owner}/{repo}/milestones", { title: "by app" });
  check(ac.status === 201 && ac.data?.creator?.login === BOT_LOGIN && ac.data?.creator?.type === "Bot", "a pull_requests-only App token creates a milestone, its creator the bot", [ac.status, ac.data?.creator?.login]);
  const ad = await call(pr, "DELETE /repos/{owner}/{repo}/milestones/{milestone_number}", { milestone_number: ac.data?.number });
  check(ad.status === 204 && (await api("GET", `${V3}/milestones/${ac.data?.number}`, admin)).status === 404 && (await gqlMilestone(ac.data?.number)) === null,
    "the same token deletes it: 204, then REST 404 and GraphQL null", ad.status);
  const co = octokit((await appToken({ contents: "write", metadata: "read" })).token);
  const cr = await call(co, "POST /repos/{owner}/{repo}/milestones", { title: "by contents" });
  check(cr.status === 403 && cr.data?.message === "Resource not accessible by integration", "a contents-only App token is refused 403", [cr.status, cr.data?.message]);
  }
  // Deletes: v2 and the label.
  const d2 = await call(o, "DELETE /repos/{owner}/{repo}/milestones/{milestone_number}", { milestone_number: m2 });
  check(d2.status === 204 && (await api("GET", `${V3}/milestones/${m2}`, admin)).status === 404 && (await gqlMilestone(m2)) === null, "issues.deleteMilestone v2: 204, REST 404, GraphQL null", d2.status);
  const dl = await call(o, "DELETE /repos/{owner}/{repo}/labels/{name}", { name: "plain2" });
  check(dl.status === 204 && (await restLabel("plain2")).status === 404 && !(await gqlLabels()).some((l) => l.name === "plain2"), "issues.deleteLabel: 204, REST 404, GraphQL gone", dl.status);
  writeFileSync(saved, JSON.stringify({ ...s, v1: m1 }));
} else if (line === "gh") {
  const s = S(), i = s.issue;
  // gh's own milestone, made over REST (gh has no milestone create).
  const gm = await api("POST", `${V3}/milestones`, admin, { title: "gh1" });
  const m = gm.body?.number;
  check(gm.status === 201, "milestone gh1 made", gm.status);
  if (LOCAL) {
    ev({ skipped: "gh's commands (F137_LOCAL); issue I gets gh1 over REST so the NOT_FOUND line has a milestone to keep" });
    await api("PATCH", `${V3}/issues/${i}`, admin, { milestone: m });
  } else {
  // 32.36 through gh's own REST calls.
  check(gh("label", "create", "q", "-c", "ededed").code === 0 && (await restLabel("q")).status === 200 && (await gqlLabels()).some((l) => l.name === "q"), "gh label create: exit 0, read back over REST and GraphQL");
  check(gh("label", "edit", "q", "-d", "by gh").code === 0 && (await restLabel("q")).body?.description === "by gh" && (await gqlLabels()).find((l) => l.name === "q")?.description === "by gh", "gh label edit: exit 0, description read back over REST and GraphQL");
  check(gh("label", "delete", "q", "--yes").code === 0 && (await restLabel("q")).status === 404 && !(await gqlLabels()).some((l) => l.name === "q"), "gh label delete: exit 0, REST 404, GraphQL gone");
  const issueIs = async (title, body, ms, what) => {
    const r = await restIssue(i), g = await gqlItem(i);
    return check(r?.title === title && r?.body === body && msOf(r) === ms && g?.title === title && g?.body === body && msOf(g) === ms, what, { r: [r?.title, r?.body, msOf(r)], g: [g?.title, g?.body, msOf(g)] });
  };
  check(gh("issue", "edit", String(i), "-t", "gh t", "-b", "gh b").code === 0, "gh issue edit -t -b: exit 0");
  await issueIs("gh t", "gh b", null, "gh issue edit -t -b read back over REST and GraphQL");
  check(gh("issue", "edit", String(i), "-m", "gh1").code === 0, "gh issue edit -m: exit 0");
  await issueIs("gh t", "gh b", m, "gh issue edit -m read back over REST and GraphQL");
  check(gh("issue", "edit", String(i), "--remove-milestone").code === 0, "gh issue edit --remove-milestone: exit 0");
  await issueIs("gh t", "gh b", null, "gh issue edit --remove-milestone read back over REST and GraphQL");
  check(gh("issue", "edit", String(i), "-t", "gh t2", "-b", "gh b2", "-m", "gh1").code === 0, "gh issue edit -t -b -m: exit 0");
  await issueIs("gh t2", "gh b2", m, "gh issue edit -t -b -m read back over REST and GraphQL (one EditWithMilestone)");
  const made = gh("issue", "create", "-t", "gh made", "-b", "b", "-m", "gh1");
  const n = Number(made.out.match(/\/issues\/(\d+)/)?.[1]);
  check(made.code === 0 && n > 0 && msOf(await restIssue(n)) === m && msOf(await gqlItem(n)) === m, "gh issue create -m: exit 0, read back over REST and GraphQL", made);
  if (s.pr) check(gh("pr", "edit", String(s.pr), "-m", "gh1").code === 0 && msOf(await restIssue(s.pr)) === m && msOf(await gqlItem(s.pr, true)) === m, "gh pr edit -m: exit 0, read back over REST and GraphQL");
  if (s.pr) check(gh("pr", "edit", String(s.pr), "--remove-milestone").code === 0 && msOf(await restIssue(s.pr)) === null && msOf(await gqlItem(s.pr, true)) === null, "gh pr edit --remove-milestone: exit 0, read back over REST and GraphQL");
  const view = gh("issue", "view", String(i), "--json", "title,body,milestone");
  let v = null; try { v = JSON.parse(view.out); } catch {}
  check(view.code === 0 && v?.title === "gh t2" && v?.milestone?.title === "gh1", "gh issue view --json reads the title and milestone", view);
  }
  // The lead's line: an unknown milestoneId through gh's issue edit -m document is NOT_FOUND and changes nothing.
  const gone = await api("POST", `${V3}/milestones`, admin, { title: "gone" });
  const goneId = gone.body?.node_id;
  await api("DELETE", `${V3}/milestones/${gone.body?.number}`, admin);
  for (const [what, id] of [["a deleted milestone's id", goneId], ["an issue's id", s.issueNode]]) {
    const before = await snapshot(i);
    const b = (await ghPost(ghDoc("IssueUpdate-1.json"), { input: { id: s.issueNode, milestoneId: id } })).body;
    ev({ unknown: what, reply: b });
    check(b?.data?.updateIssue === null && errType(b) === "NOT_FOUND", `gh's IssueUpdate with ${what} as milestoneId: NOT_FOUND`, b);
    check(await snapshot(i) === before, `... and issue I is unchanged over REST and GraphQL (milestone gh1, title, body, updated_at)`, await snapshot(i));
  }
  const nosuch = gh("issue", "edit", String(i), "-m", "nosuch");
  if (nosuch) ev({ note: "gh resolves -m by title before it sends: an unknown title fails in gh, no mutation sent", code: nosuch.code });
  // Gap probes: gh commands whose GraphQL no format serves. Each must change nothing; their answers are recorded.
  for (const args of [["issue", "lock", String(i)], ["issue", "comment", String(i), "--edit-last", "-b", "probe"], ["issue", "list", "-m", "gh1"]]) {
    const before = await snapshot(i);
    const p = gh(...args);
    if (!p) continue;
    ev({ gap: `gh ${args.join(" ")}`, code: p.code });
    check(await snapshot(i) === before, `gap probe gh ${args[0]} ${args[1]} changes nothing`, await snapshot(i));
  }
} else if (line === "delete") {
  ev({ delete: R, status: (await api("DELETE", V3, admin)).status });
} else {
  console.error("line: repo base octokit gh delete");
  process.exit(2);
}
if (line !== "delete") verdict();
