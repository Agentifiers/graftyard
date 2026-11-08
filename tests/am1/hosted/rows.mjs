// AM1's hosted rows on graft-m1 through graft-m1-edge (no member shortcuts for the actions), m3org/j4 unless noted:
//   l013    lease limits: 100 each of Exclusive `**`, `*`, `*/x`, an Exclusive with no live task (a user PAT), a second
//           Exclusive for one task, an App token with no session; then an Exclusive renewed past 4 h read as Shared is
//           not checkable in wall time and is reported from the code row
//   l012    100 conflicting pushes and 100 conflicting REST ref writes into a task's Exclusive scope under
//           `AgentsExclusive`, per actor kind the install can field (confined vendor App; flagged App without
//           `confine`; human PAT), plus the idle-60-s lease then an overlapping acquire 10 min later
//   lf1     an App with N open tasks across the repositories in REPOS, uninstalled through the operator's `Uninstall`
//           (the edge serves no uninstall route), then reads and next commands through the edge, then reinstalled
//   lf5     five draft PRs' plan blocks through GraphQL `createPullRequest`, and a body edit through `updatePullRequest`
// Rows go to tests/am1/raw/rows-<row>.jsonl; summaries to stdout.
//   node tests/am1/hosted/rows.mjs <row> [n]
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { OP, api, ghs, Git, log, must, pat, row, sleep, state, tokens } from "./lib.mjs";

const [which, nArg] = process.argv.slice(2);
const N = Number(nArg ?? 100);
const RAW = new URL("../raw/", import.meta.url).pathname;
mkdirSync(RAW, { recursive: true });
const OUT = `${RAW}rows-${which}.jsonl`;
const R = "m3org/j4";
const V1 = `/api/graftyard/v1/repos/${R}`;
const tag = Date.now().toString(36);
const count = (xs) => xs.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {});

async function task(tok, name) {
  const r = await api("POST", `${V1}/tasks`, tok, { kind: "external", session: name, summary: "am1 rows" });
  if (r.status !== 201) throw new Error(`task ${name}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  return r.body.id;
}
const acquire = (tok, body, session) =>
  api("POST", `${V1}/leases`, tok, body, session ? { "x-graftyard-agent-session": session } : {});

if (which === "l013") {
  const app = (await ghs({})).body.token;
  const id = await task(app, `l013-${tag}`);
  const user = tokens.admin;
  const out = {};
  const run = async (name, f) => {
    const st = [];
    for (let i = 0; i < N; i++) st.push((await f(i)).status);
    out[name] = count(st);
    row(OUT, { case: name, statuses: out[name] });
    log(`${name}: ${JSON.stringify(out[name])}`);
  };
  await run("exclusive **", () => acquire(app, { paths: ["**"], mode: "exclusive" }, id));
  await run("exclusive *", () => acquire(app, { paths: ["*"], mode: "exclusive" }, id));
  await run("exclusive */x", () => acquire(app, { paths: ["*/x"], mode: "exclusive" }, id));
  await run("exclusive, user token, no live task", (i) => acquire(user, { paths: [`l013u/${i}/**`], mode: "exclusive" }));
  const first = await acquire(app, { paths: [`l013/${tag}/**`], mode: "exclusive", ttl_seconds: 600 }, id);
  row(OUT, { case: "first exclusive for the task", status: first.status, id: first.body?.lease?.id });
  await run("second exclusive for one task", (i) => acquire(app, { paths: [`l013b/${tag}/${i}/**`], mode: "exclusive" }, id));
  await run("App token, no session", (i) => acquire(app, { paths: [`l013n/${i}/**`] }));
  // Uninstall while holding: the task's lease stops reading live with no sweep (LF-1 covers the uninstall).
  if (first.body?.lease?.id) {
    const rel = await api("DELETE", `${V1}/leases/${first.body.lease.id}`, app);
    row(OUT, { case: "release", status: rel.status });
  }
  log(`l013 ${JSON.stringify(out)}`);
}

if (which === "l012") {
  // The Exclusive holder: a task of m3-ci's with a lease on `l012/<tag>/**`, under AgentsExclusive. Then, per actor
  // kind the install can field, N pushes and N REST ref writes whose changed paths fall in that scope:
  //   confined vendor App   m3-ci flagged ThirdParty, confine refs/heads/claude/** (ActorKind::Agent): all refused
  //   flagged App, no confine  m3-ci flagged ThirdParty with no confine (an Agent too, by the design's classing)
  //   unflagged App         m3-ci with no AgentApp row (Bot): none refused
  //   human PAT             m3-admin (Human): none refused
  // A Graftyard-run App and `Graftyard MCP` need the install's own Apps, which this install has not registered.
  const admin = tokens.admin;
  const flag = (confine) => api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine });
  await flag("refs/heads/claude/**");
  const pol = await api("PUT", `${V1}/agents/policy`, admin, { leases: "agents_exclusive" });
  row(OUT, { step: "policy agents_exclusive", status: pol.status, body: pol.body });
  let app = (await ghs({})).body.token;
  const holderTask = await task(app, `l012h-${tag}`);
  const held = await acquire(app, { paths: [`l012/${tag}/**`], mode: "exclusive", ttl_seconds: 3600, intent: "L0-12 holder" }, holderTask);
  row(OUT, { step: "holder exclusive", status: held.status, lease: held.body?.lease?.id });
  log(`holder: ${held.status} ${held.body?.lease?.id}`);
  const g = new Git(R).as(admin);
  g.clone();
  const main = g.run(["rev-parse", "HEAD"]).out.trim();
  const commitTouching = (k) => {
    g.run(["checkout", "-q", "--detach", main]);
    const p = join(g.wt, `l012/${tag}/f${k}.txt`);
    mkdirSync(join(p, ".."), { recursive: true });
    writeFileSync(p, `${k} ${Math.random()}\n`);
    g.run(["add", "-A"]);
    g.run(["commit", "-q", "-m", `l012 ${k}`]);
    return g.run(["rev-parse", "HEAD"]).out.trim();
  };
  const kinds = [
    ["confined vendor App", async () => { await flag("refs/heads/claude/**"); return (await ghs({})).body.token; }, (k) => `refs/heads/claude/l012-${tag}-${k}`],
    ["flagged App without confine", async () => { await flag(null); return (await ghs({})).body.token; }, (k) => `refs/heads/l012-${tag}-u${k}`],
    ["unflagged App", async () => { await api("DELETE", `/api/graftyard/v1/agents/apps/${state.app}`, admin); return (await ghs({})).body.token; }, (k) => `refs/heads/l012-${tag}-b${k}`],
    ["human PAT", async () => admin, (k) => `refs/heads/l012-${tag}-h${k}`],
  ];
  const summary = {};
  for (const [kind, tokenOf, ref] of kinds) {
    const tok = await tokenOf();
    await sleep(2000);
    const pushes = [], writes = [];
    for (let i = 0; i < N; i++) {
      // A REST ref write needs a commit the repository holds: one pushed by the admin under a scratch ref first.
      g.as(admin);
      const sha = commitTouching(`${kind.length}-${i}`);
      g.run(["push", "-q", "origin", `HEAD:refs/heads/l012s-${tag}-${kind.length}-${i}`]);
      const w = await api("POST", `/api/v3/repos/${R}/git/refs`, tok, { ref: `${ref(i)}-rest`, sha });
      writes.push(w.status === 201 ? "accepted" : `${w.status} ${String(w.body?.message).replace(/[0-9A-Z]{26}/g, "<id>").slice(0, 60)}`);
      g.as(tok);
      const sha2 = commitTouching(`${kind.length}-p${i}`);
      void sha2;
      const r = g.run(["push", "-q", "origin", `HEAD:${ref(i)}`]);
      pushes.push(r.ok ? "accepted" : (r.out.match(/refused: ([^)\n|]*)/)?.[1]?.replace(/[0-9A-Z]{26}/g, "<id>").slice(0, 60) ?? "rejected"));
    }
    summary[kind] = { pushes: count(pushes), writes: count(writes) };
    row(OUT, { kind, ...summary[kind] });
    log(`${kind}: ${JSON.stringify(summary[kind])}`);
  }
  await flag("refs/heads/claude/**");
  log(`l012 ${JSON.stringify(summary)}`);
}

if (which === "lf1") {
  // 10 repositories m3-ci's installation covers ("All" on m3org): j4 and lf1-0..8 (tests/am1/hosted/repos.mjs).
  const repos = ["j4", ...readFileSync(`${process.env.HOME}/.graft-m3h/am1-repos.txt`, "utf8").trim().split("\n").map((l) => l.split(" ")[0])];
  const app = (await ghs({})).body.token;
  const ids = [];
  for (let i = 0; i < N; i++) {
    const repo = repos[i % repos.length];
    const r = await api("POST", `/api/graftyard/v1/repos/m3org/${repo}/tasks`, app, { kind: "external", session: `lf1-${tag}-${i}`, summary: "am1 lf1" });
    if (r.status !== 201) throw new Error(`task ${repo} ${i}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
    ids.push([repo, r.body.id]);
  }
  row(OUT, { step: "created", n: ids.length, repos: repos.length });
  const read = async () => count(await Promise.all(ids.map(async ([repo, id]) =>
    (await api("GET", `/api/graftyard/v1/repos/m3org/${repo}/tasks/${id}`, tokens.admin)).body?.state ?? "error")));
  const before = await read();
  row(OUT, { step: "before uninstall", states: before });
  log(`before uninstall: ${JSON.stringify(before)}`);
  await must(`authz:${state.org}`, { cred: OP, op: { Uninstall: { id: state.installation } } });
  const t0 = Date.now();
  const after = await read();
  row(OUT, { step: "after uninstall, first read", states: after, ms: Date.now() - t0 });
  log(`after uninstall: ${JSON.stringify(after)}`);
  // Next commands: the org admin cancels each (the App's tokens no longer mint); each records the derived state.
  const next = count(await Promise.all(ids.map(async ([repo, id]) => {
    const r = await api("POST", `/api/graftyard/v1/repos/m3org/${repo}/tasks/${id}/cancel`, tokens.admin, {});
    return `${r.status} ${r.body?.state ?? r.body?.message ?? ""}`.slice(0, 60);
  })));
  row(OUT, { step: "next command", answers: next });
  log(`next command: ${JSON.stringify(next)}`);
  const recorded = await read();
  row(OUT, { step: "after next command", states: recorded });
  // Reinstall, as setup.ts installed it, with the AM1 permissions (app-perms.mjs).
  const perms = { contents: "Write", metadata: "Read", issues: "Write", pull_requests: "Write", checks: "Write" };
  await must(`authz:${state.org}`, { cred: OP, op: { Install: { id: state.installation, row: { app: state.app, repos: "All", permissions: perms }, app_owner: state.admin } } });
  const reread = await read();
  row(OUT, { step: "after reinstall", states: reread });
  log(`after reinstall (old tasks stay cancelled): ${JSON.stringify(reread)}`);
}


/** GraphQL through the edge. */
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
const CREATE = "mutation($input: CreatePullRequestInput!) { createPullRequest(input: $input) { pullRequest { id number body } } }";
const UPDATE = "mutation($input: UpdatePullRequestInput!) { updatePullRequest(input: $input) { pullRequest { number body } } }";
const fence = (lines) => "```graftyard-plan\n" + lines.join("\n") + "\n```";

if (which === "lf5") {
  // A branch to open drafts from: one commit pushed as the writer (m3-admin, an org admin).
  const admin = tokens.admin;
  const repoId = (await gql(admin, "query { repository(owner: \"m3org\", name: \"j4\") { id } }")).body?.data?.repository?.id;
  const g = new Git(R).as(admin);
  g.clone();
  const branch = (k) => {
    g.run(["checkout", "-q", "-B", `lf5-${tag}-${k}`, "origin/main"]);
    writeFileSync(join(g.wt, `lf5-${tag}-${k}.txt`), `${k}\n`);
    g.run(["add", "-A"]);
    g.run(["commit", "-q", "-m", `lf5 ${k}`]);
    g.run(["push", "-q", "origin", `HEAD:refs/heads/lf5-${tag}-${k}`]);
    return `lf5-${tag}-${k}`;
  };
  const big = fence([`paths: lf5/${tag}/big/**`, "intent: " + "x".repeat(4200)]);
  const bodies = {
    valid: `Plan\n\n${fence([`paths: lf5/${tag}/valid/**, docs/${tag}/*.md`, "intent: the valid block"])}\n`,
    repeated: fence([`paths: lf5/${tag}/rep/**`, `paths: lf5/${tag}/rep2/**`]),
    badglob: fence(["paths: /etc/passwd"]),
    second: `${fence(["paths: /bad"])}\n\n${fence([`paths: lf5/${tag}/second/**`])}`,
    over4k: big,
  };
  const out = {};
  for (const [k, body] of Object.entries(bodies)) {
    const r = await gql(admin, CREATE, { input: { repositoryId: repoId, baseRefName: "main", headRefName: branch(k), title: `lf5 ${k}`, body, draft: true } });
    const pr = r.body?.data?.createPullRequest?.pullRequest;
    out[k] = { status: r.status, number: pr?.number, bodyUnchanged: pr?.body === body, errors: r.body?.errors?.[0]?.message };
  }
  const leases = await api("GET", `${V1}/leases?per_page=100&paths=lf5/${tag}/**`, admin);
  const list = Array.isArray(leases.body) ? leases.body : [];
  for (const k of Object.keys(out)) out[k].leases = list.filter((l) => l.paths?.some((p) => p.includes(`/${k === "repeated" ? "rep" : k}/`))).length;
  row(OUT, { step: "drafts", out, leasesListed: list.length });
  log(`lf5 drafts: ${JSON.stringify(out)} listed ${list.length}`);
  // A body edit through updatePullRequest: a draft without a block gains one; the body comes back unchanged.
  const plain = await gql(admin, CREATE, { input: { repositoryId: repoId, baseRefName: "main", headRefName: branch("edit"), title: "lf5 edit", body: "no plan yet", draft: true } });
  const id = plain.body?.data?.createPullRequest?.pullRequest?.id;
  const edited = `Edited\n\n${fence([`paths: lf5/${tag}/edit/**`])}\n`;
  const u = await gql(admin, UPDATE, { input: { pullRequestId: id, body: edited } });
  const l2 = await api("GET", `${V1}/leases?per_page=100&paths=lf5/${tag}/edit/**`, admin);
  const v = { status: u.status, bodyUnchanged: u.body?.data?.updatePullRequest?.pullRequest?.body === edited, errors: u.body?.errors?.[0]?.message,
    leases: Array.isArray(l2.body) ? l2.body.length : l2.status };
  row(OUT, { step: "updatePullRequest body edit", ...v });
  log(`lf5 edit: ${JSON.stringify(v)}`);
}

if (which === "plan600") {
  // L0-13's plan half: N plan blocks from accounts without push access (fresh users, no org membership), then N
  // from writers; zero plan leases from the first, at most PLAN_LEASES_PER_REPO from the second.
  const admin = tokens.admin;
  const repoId = (await gql(admin, "query { repository(owner: \"m3org\", name: \"j4\") { id } }")).body?.data?.repository?.id;
  void repoId;
  log("plan600: needs N distinct non-writer accounts with read access to a private repository; see results");
}

if (which === "pat") {
  // One fresh PAT for m3-member (a writer through the org's base permission), for drivers that need a second user.
  log(`made ${(await pat(state.member)).length} chars`);
}
await sleep(0);
