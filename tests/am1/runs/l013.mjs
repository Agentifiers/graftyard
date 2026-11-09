// L0-13's halves that `rows.mjs l013` left (`build/BUILD-PLAN.md`, L0-13), through graft-m2b's edge:
//
//   plan     N draft PRs with a `graftyard-plan` block from accounts without push access (fresh users, no org
//            membership, Read on a public repository), then N from writers (fresh users made org members, base
//            Write). Count the plan leases each half got (`GET .../leases`): zero for non-writers, at most
//            PLAN_LEASES_PER_REPO (500) for writers. The writers' blocks are one per draft, LEASES_PER_HOLDER (8) per
//            user, so writers = ceil(N / 8).
//   demote   A writer (fresh user, org member) holds an Exclusive lease through an external task of m3-ci's with that
//            user as requester; the user is removed from the org (`SetMember role: null`), and an agent push into the
//            scope under AgentsExclusive is no longer refused, while the lease list stops showing the lease.
//   renew    An Exclusive lease renewed every 50 min for `hours` (default 5): its mode as the list reads it, every
//            renewal, until past EXCLUSIVE_MAX (4 h), where it must read `shared`.
//
// Users and their PATs are made as the operator (the account rows as tests/am1/rest/a1.mjs seeds requesters; PATs by
// lib.mjs `pat`), ids from 7100100. No token reaches argv or a log.
//   node tests/am1/runs/l013.mjs plan <repo> [N]
//   node tests/am1/runs/l013.mjs demote <repo>
//   node tests/am1/runs/l013.mjs renew <repo> [hours]
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, OP, accountShard, api, ghs, log, loginShard, must, pat, row, sleep, state, tokens } from "../hosted/lib.mjs";

const [which, repo, nArg] = process.argv.slice(2);
if (!["plan", "demote", "renew"].includes(which) || !repo) throw new Error("usage: l013.mjs <plan|demote|renew> <repo> [n]");
const R = `m3org/${repo}`;
const V1 = `/api/graftyard/v1/repos/${R}`;
const OUT = new URL(`../raw/runs/l013-${which}.jsonl`, import.meta.url).pathname;
const tag = Date.now().toString(36);
const count = (xs) => xs.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {});
const admin = tokens.admin;
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
const CREATE = "mutation($input: CreatePullRequestInput!) { createPullRequest(input: $input) { pullRequest { id number } } }";
const fence = (lines) => "```graftyard-plan\n" + lines.join("\n") + "\n```";

/** A fresh user with a login and an Identity chain, and a PAT; `member` makes it an org member (base Write). */
async function user(k, member) {
  const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
  const [a, login] = [7100100 + k, `l013-u${k}`];
  await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind: "User", account: a } } }, "Taken");
  await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
  await must(`authz:${a}`, { cred: OP, op: { Open: { kind: "User", enterprise: null } } }, "AlreadyOpened");
  await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind: "User", created: Date.now() } } }, "AlreadyCreated");
  await must(`authz:${state.org}`, { cred: OP, op: { SetMember: { user: a, role: member ? "Member" : null } } });
  return { id: a, login, token: await pat(a, `l013 ${tag}`) };
}

const g = new Git(R).as(admin);
g.clone();
const main = g.run(["rev-parse", "HEAD"]).out.trim();
/** A branch the admin pushed, one commit on main, for a draft to open from. */
function branch(name) {
  g.run(["checkout", "-q", "--detach", main]);
  const p = join(g.wt, "l013", tag, `${name}.txt`);
  mkdirSync(join(p, ".."), { recursive: true });
  writeFileSync(p, `${name}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `l013 ${name}`]);
  const r = g.run(["push", "-q", "origin", `HEAD:refs/heads/${name}`]);
  return r.ok ? name : null;
}
const repoId = (await gql(admin, `query { repository(owner: "m3org", name: "${repo}") { id } }`)).body?.data?.repository?.id;
const leases = async (glob) => {
  const out = [];
  // The lease list pages by `after` (the last id of the page before), not `page`.
  for (let after = null; ;) {
    const r = await api("GET", `${V1}/leases?per_page=100${after ? `&after=${after}` : ""}&paths=${encodeURIComponent(glob)}`, admin);
    if (!Array.isArray(r.body)) return { status: r.status, list: out };
    out.push(...r.body);
    if (r.body.length < 100) break;
    after = r.body[r.body.length - 1].id;
  }
  return { status: 200, list: out };
};

if (which === "plan") {
  const N = Number(nArg ?? 600);
  const half = async (kind, users, perUser) => {
    const st = [];
    for (let i = 0; i < N; i++) {
      const u = users[Math.floor(i / perUser) % users.length];
      const b = branch(`l013-${tag}-${kind}-${i}`);
      if (!b) { st.push("branch push failed"); continue; }
      const body = `Plan\n\n${fence([`paths: l013/${tag}/${kind}/${i}/**`, `intent: L0-13 ${kind} ${i}`])}\n`;
      const r = await gql(u.token, CREATE, { input: { repositoryId: repoId, baseRefName: "main", headRefName: b, title: `l013 ${kind} ${i}`, body, draft: true } });
      st.push(r.body?.data?.createPullRequest?.pullRequest ? "opened" : `gql ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 60)}`);
    }
    const l = await leases(`l013/${tag}/${kind}/**`);
    const plan = l.list.filter((x) => x.paths?.some((p) => p.startsWith(`l013/${tag}/${kind}/`)));
    const v = { kind, drafts: count(st), planLeases: plan.length, listStatus: l.status, holders: count(plan.map((x) => x.holder?.login ?? x.holder?.type ?? "?")) };
    row(OUT, v);
    log(`plan ${kind}: ${JSON.stringify(v)}`);
    return v;
  };
  const readers = [await user(0, false), await user(1, false), await user(2, false)];
  const writers = [];
  for (let k = 0; k < Math.ceil(N / 8); k++) writers.push(await user(10 + k, true));
  log(`users: ${readers.length} non-writers, ${writers.length} writers`);
  const nonw = await half("nonwriter", readers, Math.ceil(N / readers.length));
  const wr = await half("writer", writers, 8);
  log(`l013 plan: nonwriters ${nonw.planLeases} plan leases, writers ${wr.planLeases} (cap 500)`);
}

if (which === "demote") {
  const holder = await user(5, true);
  const pol = await api("PUT", `${V1}/agents/policy`, admin, { leases: "agents_exclusive" });
  await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  await sleep(2000);
  const app = (await ghs({})).body.token;
  const t = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `l013d-${tag}`, summary: "l013 demote", requester: holder.login });
  const held = await api("POST", `${V1}/leases`, holder.token, { paths: [`l013d/${tag}/**`], mode: "exclusive", ttl_seconds: 3600 }, { "x-graftyard-agent-session": t.body?.id ?? "" });
  row(OUT, { step: "setup", policy: pol.status, task: t.status, lease: held.status });
  const pushInto = (k) => {
    g.as(app);
    g.run(["checkout", "-q", "--detach", main]);
    const p = join(g.wt, "l013d", tag, `f${k}.txt`);
    mkdirSync(join(p, ".."), { recursive: true });
    writeFileSync(p, `${k}\n`);
    g.run(["add", "-A"]);
    g.run(["commit", "-q", "-m", `l013d ${k}`]);
    const r = g.run(["push", "-q", "origin", `HEAD:refs/heads/claude/l013d-${tag}-${k}`]);
    return r.ok ? "accepted" : (r.out.match(/refused: ([^)\n|]*)/)?.[1]?.replace(/[0-9A-Z]{26}/g, "<id>").slice(0, 50) ?? "rejected");
  };
  const before = [pushInto("b0"), pushInto("b1")];
  const listedBefore = (await leases(`l013d/${tag}/**`)).list.length;
  await must(`authz:${state.org}`, { cred: OP, op: { SetMember: { user: holder.id, role: null } } });
  const after = [pushInto("a0"), pushInto("a1")];
  const listedAfter = (await leases(`l013d/${tag}/**`)).list.length;
  const back = await api("PUT", `${V1}/agents/policy`, admin, { leases: "advisory" });
  const v = { lease: held.status, pushesBefore: count(before), listedBefore, pushesAfterRemoval: count(after), listedAfter, policyBack: back.status };
  row(OUT, v);
  log(`l013 demote: ${JSON.stringify(v)}`);
}

if (which === "renew") {
  const hours = Number(nArg ?? 5);
  await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  await sleep(2000);
  let app = (await ghs({})).body.token;
  const t = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `l013r-${tag}`, summary: "l013 renew" });
  const h = () => ({ "x-graftyard-agent-session": t.body?.id ?? "" });
  const a = await api("POST", `${V1}/leases`, app, { paths: [`l013r/${tag}/**`], mode: "exclusive", ttl_seconds: 3600 }, h());
  const id = a.body?.lease?.id;
  row(OUT, { step: "acquire", status: a.status, mode: a.body?.lease?.mode, granted: a.body?.lease?.granted_at ?? null });
  log(`renew: acquire ${a.status} ${a.body?.lease?.mode}`);
  const t0 = Date.now();
  while (Date.now() - t0 < hours * 3_600_000 + 10 * 60_000) {
    await sleep(50 * 60_000);
    app = (await ghs({})).body.token;
    // Keep the task from TASK_IDLE: one summary edit before each renewal.
    await api("PATCH", `${V1}/tasks/${t.body?.id}`, app, { summary: `l013 renew ${Date.now()}` });
    const r = await api("POST", `${V1}/leases/${id}/renew`, app, { ttl_seconds: 3600 }, h());
    const l = (await leases(`l013r/${tag}/**`)).list.find((x) => x.id === id);
    const v = { at_min: Math.round((Date.now() - t0) / 60_000), renew: r.status, mode: r.body?.lease?.mode ?? r.body?.mode ?? null, listed: l?.mode ?? null };
    row(OUT, v);
    log(`renew: ${JSON.stringify(v)}`);
  }
}
