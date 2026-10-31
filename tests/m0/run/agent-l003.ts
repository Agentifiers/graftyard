// M0-WP13, L0-03: token ceilings with raw tokens, bypassing the router (the Sandbox's outbound handler is not in the
// path: the driver holds the minted tokens and talks to the host directly). Tokens come from the real token request,
// `POST /app/installations/{id}/access_tokens` with `graftyard_task`, signed by the App's JWT. 100 attempts per case,
// over git (real `git push`), REST, and GraphQL where the case has those forms:
//   - a workspace token pushes to the base;
//   - a workspace token pushes to another task's prefix;
//   - a workspace token pushes to `.github/workflows/`;
//   - a token minted with no live AgentTask pushes (a token minted with no `graftyard_task`, and one whose task has
//     since completed or timed out);
//   - a fork-mode token pushes to its fork after its task is cancelled;
//   - the base token (graftyard-agent's, with the task claim) merges.
// Controls: the same workspace token pushes inside its prefix on the fork, before the cancel.
//
//   node run/agent-l003.ts [--local] [--url U] [--n 100] [--k 8] [--run X]

import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { Work, baseUrl, push, type PushResult } from "./agent-lib.ts";
import { startLocal } from "./agent.ts";
import { RAW, Rows, V1, api, cell, harness, msg, orgSet, tok, type Attempt } from "./agent-lib13.ts";
import { INST } from "../src/agent/fixture.ts";

const { values: opt } = parseArgs({ options: { local: { type: "boolean", default: false }, url: { type: "string" }, n: { type: "string", default: "100" },
  k: { type: "string", default: "8" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const N = Number(opt.n), K = Number(opt.k);
const local = opt.local ? await startLocal() : null;
const U = local ? local.url : baseUrl(opt);
const rows = new Rows("l003", opt.run);
const W = new Work(`l003-${opt.run}`);
await W.init();
const git = (repo: string) => `${U}/${repo}.git`;
const BASE = "acme/mono", FORK = "acme-agents/mono";
console.log(`L0-03 against ${U}, run ${opt.run}`);
await harness(U, "/m0/setup", {});
await orgSet(U, { agents_enabled: true });

const T = { alice: await tok("alice"), bob: await tok("bob"), carol: await tok("carol"), jwtWs: await tok("jwt-workspace"), jwtAgent: await tok("jwt-agent") };
const must = (r: PushResult, what: string) => { if (r.code !== 0) throw new Error(`${what}: ${r.err}`); };
const main = await W.commit(null, { "README.md": "mono\n", "src/a.txt": "a\n", ".github/workflows/ci.yml": "on: push\n" }, "init\n");
must(await push(W, git(BASE), T.alice, [`${main}:refs/heads/main`]), "seed main");
await harness(U, "/m0/fork-sync", {});

// Three tasks by human writers: A (the agent under test), B (another task), C (cancelled later), D (to complete).
const mk = async (who: string, prompt: string) => {
  const r = await api(U, "POST", `${V1}/tasks`, who, { prompt });
  if (r.status !== 201) throw new Error(`create: ${r.status} ${JSON.stringify(r.body)}`);
  return r.body.id as string;
};
const A = await mk(T.bob, "task A"), B = await mk(T.carol, "task B"), C = await mk(T.bob, "task C"), D = await mk(T.carol, "task D");
const mint = async (jwt: string, inst: number, repos: string[], task: string | null) => {
  const r = await api(U, "POST", `/api/v3/app/installations/${inst}/access_tokens`, jwt, { repositories: repos, ...(task ? { graftyard_task: task } : {}) });
  if (r.status !== 201) throw new Error(`mint ${task}: ${r.status} ${JSON.stringify(r.body)}`);
  return r.body.token as string;
};
const wsA = await mint(T.jwtWs, INST.workspace, [FORK], A);
const wsC = await mint(T.jwtWs, INST.workspace, [FORK], C);
const wsD = await mint(T.jwtWs, INST.workspace, [FORK], D);
const wsNone = await mint(T.jwtWs, INST.workspace, [FORK], null);
const agentA = await mint(T.jwtAgent, INST.agent, [BASE], A);
// A widened graftyard-workspace installation that also covers the base (bad config), so the base push reaches decide.
const wideA = await mint(await tok("jwt-workspace"), INST.wide, [BASE, FORK], A);
// Likewise graftyard-agent widened to every permission, so a merge by the base token reaches `decide`.
const agentWideA = await mint(T.jwtAgent, INST.agentWide, [BASE], A);
console.log(`tasks A ${A.slice(0, 8)} B ${B.slice(0, 8)} C ${C.slice(0, 8)} D ${D.slice(0, 8)}; tokens minted through access_tokens with graftyard_task`);

const PA = `refs/heads/graftyard/${A}/`, PB = `refs/heads/graftyard/${B}/`, PC = `refs/heads/graftyard/${C}/`, PD = `refs/heads/graftyard/${D}/`;
// Controls before anything changes: A and C push in their prefixes on the fork; the agent opens a draft PR on the base.
const okA = await W.commit(main, { "src/a.txt": "a1\n" }, "A work\n");
must(await push(W, git(FORK), wsA, [`${okA}:${PA}work`]), "control: A pushes in its prefix on the fork");
must(await push(W, git(FORK), wsC, [`${okA}:${PC}work`]), "control: C pushes before cancel");
must(await push(W, git(FORK), wsD, [`${okA}:${PD}work`]), "control: D pushes before completion");
// The PR's head lives on the base for the merge case: a human (bob, A's requester) pushes A's branch to the base.
must(await push(W, git(BASE), T.bob, [`${okA}:${PA}work`]), "base copy of A's branch");
const pr = await api(U, "POST", `/api/v3/repos/${BASE}/pulls`, agentA, { head: `graftyard/${A}/work`, base: "main", draft: true, title: "A" }, { "X-Graftyard-Agent-Session": A });
if (pr.status !== 201) throw new Error(`PR: ${JSON.stringify(pr.body)}`);
// Then: C is cancelled by its requester, D is completed by its App.
const cc = await api(U, "POST", `${V1}/tasks/${C}/cancel`, T.bob, {});
const dd = await api(U, "PATCH", `${V1}/tasks/${D}`, agentA, { state: "completed" });
if (cc.status !== 200 || dd.status !== 200) throw new Error(`cancel/complete: ${cc.status} ${dd.status} ${msg(dd.body)}`);

const gitA = (r: PushResult): Attempt => {
  const bad = r.refs.find((x) => x.flag === "!");
  if (!r.refs.length) return { refused: true, status: /error: (\d{3})|returned error: (\d{3})/.exec(r.err)?.slice(1).find(Boolean) ?? `exit ${r.code}`, text: r.err.replace(/\s+/g, " ").slice(-160), ms: r.ms };
  return { refused: !!bad, status: bad ? "ng" : "ok", text: bad ? bad.summary : "ok", ms: r.ms };
};
const restA = (r: { status: number; body: any; ms: number }): Attempt => ({ refused: r.status >= 400, status: String(r.status), text: msg(r.body), ms: r.ms });
const gqlA = (r: { status: number; body: any; ms: number }): Attempt => {
  const e = r.body?.errors?.[0];
  return { refused: r.status >= 400 || !!e, status: e ? `${r.status} ${e.type}` : String(r.status), text: e ? e.message : JSON.stringify(r.body?.data).slice(0, 120), ms: r.ms };
};
const fresh = (tag: string, files: (i: number) => Record<string, string>) => (i: number) => W.commit(okA, files(i), `${tag} ${i}\n`);
const gpush = (repo: string, token: string, spec: (i: number) => Promise<string>) =>
  async (i: number, probe: string) => gitA(await push(W, git(repo), token, [await spec(i)], { probe }));
const rest = (method: string, path: string, token: string, body: (i: number) => unknown) =>
  async (i: number, probe: string) => restA(await api(U, method, path, token, body(i), { "x-m0-probe": probe }));
const gql = (token: string, m: string, input: (i: number) => unknown) =>
  async (i: number, probe: string) => gqlA(await api(U, "POST", "/api/graphql", token,
    { query: `mutation($input: ${m[0].toUpperCase()}${m.slice(1)}Input!) { ${m}(input: $input) { clientMutationId } }`, variables: { input: input(i) } }, { "x-m0-probe": probe }));
const c = (name: string, f: (i: number, probe: string) => Promise<Attempt>, expect: "refused" | "accepted" = "refused", n = N) => cell(rows, U, opt.run!, name, expect, n, K, f);

// 1. A workspace token pushes to the base. Shipped: graftyard-workspace is installed only on the workspace org.
const plain = fresh("base", (i) => ({ "src/b.txt": `${i}\n` }));
await c("ws token -> base, main (git)", gpush(BASE, wsA, async (i) => `${await plain(i)}:refs/heads/main`));
await c("ws token -> base, its own prefix (git)", gpush(BASE, wsA, async (i) => `${await plain(i)}:${PA}b-${i}`));
await c("ws token -> base, POST /git/refs (REST)", rest("POST", `/api/v3/repos/${BASE}/git/refs`, wsA, (i) => ({ ref: `${PA}r-${i}`, sha: okA })));
await c("ws token -> base, createRef (GraphQL)", gql(wsA, "createRef", (i) => ({ repositoryId: `R_${BASE}`, name: `${PA}g-${i}`, oid: okA })));
await c("widened ws token -> base main (git, decide)", gpush(BASE, wideA, async (i) => `${await plain(i)}:refs/heads/main`));
// 2. To another task's prefix (on its own fork).
await c("ws token -> another task's prefix (git)", gpush(FORK, wsA, async (i) => `${await plain(i)}:${PB}x-${i}`));
await c("ws token -> another task's prefix (REST)", rest("POST", `/api/v3/repos/${FORK}/git/refs`, wsA, (i) => ({ ref: `${PB}r-${i}`, sha: okA })));
await c("ws token -> another task's prefix (GraphQL)", gql(wsA, "createRef", (i) => ({ repositoryId: `R_${FORK}`, name: `${PB}g-${i}`, oid: okA })));
// 3. To .github/workflows/, inside its own prefix.
const wf = fresh("wf", (i) => ({ ".github/workflows/agent.yml": `on: push # ${i}\n` }));
await c("ws token -> .github/workflows/ (git)", gpush(FORK, wsA, async (i) => `${await wf(i)}:${PA}wf-${i}`));
await c("ws token -> .github/workflows/ (GraphQL createCommitOnBranch)", gql(wsA, "createCommitOnBranch", (i) => ({ branch: { repositoryNameWithOwner: FORK, branchName: `graftyard/${A}/work` },
  expectedHeadOid: okA, message: { headline: `wf ${i}` }, fileChanges: { additions: [{ path: ".github/workflows/x.yml", contents: Buffer.from(`on: push # ${i}\n`).toString("base64") }] } })));
const wfOne = await wf(10_000);
// The workflow commit already exists on the fork, under a human's branch (alice is the fork's admin in the bed).
must(await push(W, git(BASE), T.alice, [`${wfOne}:refs/heads/alice/wf`]), "alice's workflow commit");
await c("ws token -> .github/workflows/ (REST, existing commit)", rest("POST", `/api/v3/repos/${FORK}/git/refs`, wsA, (i) => ({ ref: `${PA}wr-${i}`, sha: wfOne })));
// 4. A token minted with no live AgentTask.
await c("token minted with no graftyard_task pushes (git)", gpush(FORK, wsNone, async (i) => `${await plain(i)}:${PA}n-${i}`));
await c("token minted with no graftyard_task pushes (REST)", rest("POST", `/api/v3/repos/${FORK}/git/refs`, wsNone, (i) => ({ ref: `${PA}nr-${i}`, sha: okA })));
await c("token for a completed task pushes (git)", gpush(FORK, wsD, async (i) => `${await plain(i)}:${PD}d-${i}`));
await c("token for a completed task pushes (GraphQL)", gql(wsD, "createRef", (i) => ({ repositoryId: `R_${FORK}`, name: `${PD}g-${i}`, oid: okA })));
// 5. A fork-mode token pushes to its fork after its task is cancelled.
await c("fork token after its task is cancelled (git)", gpush(FORK, wsC, async (i) => `${await plain(i)}:${PC}c-${i}`));
await c("fork token after its task is cancelled (REST)", rest("POST", `/api/v3/repos/${FORK}/git/refs`, wsC, (i) => ({ ref: `${PC}cr-${i}`, sha: okA })));
await c("fork token after its task is cancelled (GraphQL)", gql(wsC, "createRef", (i) => ({ repositoryId: `R_${FORK}`, name: `${PC}cg-${i}`, oid: okA })));
// 6. The base token merges: REST and GraphQL merge, POST /merges, a git fast-forward of main, and PATCH /git/refs.
const n = pr.body.number;
await c("base token merges (REST PUT merge)", rest("PUT", `/api/v3/repos/${BASE}/pulls/${n}/merge`, agentA, () => ({})));
await c("base token merges (GraphQL mergePullRequest)", gql(agentA, "mergePullRequest", () => ({ pullRequestId: `PR_${n}` })));
await c("base token merges (REST POST /merges)", rest("POST", `/api/v3/repos/${BASE}/merges`, agentA, () => ({ base: "main", head: `graftyard/${A}/work` })));
await c("base token merges (PATCH /git/refs main)", rest("PATCH", `/api/v3/repos/${BASE}/git/refs/heads/main`, agentA, () => ({ sha: okA })));
await c("base token merges (git push PR head to main)", gpush(BASE, agentA, async () => `${okA}:refs/heads/main`));
for (const [nm, f] of [["REST PUT merge", rest("PUT", `/api/v3/repos/${BASE}/pulls/${n}/merge`, agentWideA, () => ({}))],
                       ["GraphQL mergePullRequest", gql(agentWideA, "mergePullRequest", () => ({ pullRequestId: `PR_${n}` }))],
                       ["REST POST /merges", rest("POST", `/api/v3/repos/${BASE}/merges`, agentWideA, () => ({ base: "main", head: `graftyard/${A}/work` }))],
                       ["PATCH /git/refs main", rest("PATCH", `/api/v3/repos/${BASE}/git/refs/heads/main`, agentWideA, () => ({ sha: okA }))],
                       ["git push PR head to main", gpush(BASE, agentWideA, async () => `${okA}:refs/heads/main`)]] as const)
  await c(`widened base token merges (${nm}, decide)`, f);
await c("base token marks ready (GraphQL)", gql(agentA, "markPullRequestReadyForReview", () => ({ pullRequestId: `PR_${n}` })));
// Controls after: A still pushes in its prefix on the fork.
await c("control: A pushes in its prefix on the fork", gpush(FORK, wsA, async (i) => `${await plain(i)}:${PA}ok-${i}`), "accepted", 10);

const bad = rows.bad();
console.log(`\ncells off their expectation: ${bad.length}`);
for (const b of bad) console.log(`  ${b}`);
const pr2 = await harness(U, `/m0/pr?repo=${encodeURIComponent(BASE)}&n=${n}`);
console.log(`PR #${n} after: open=${pr2?.open} merged=${pr2?.merged}; main=${(await api(U, "GET", `/api/v3/repos/${BASE}/git/ref/heads/main`, T.alice)).body.object.sha.slice(0, 7)} (seed ${main.slice(0, 7)})`);
writeFileSync(`${RAW}/wp13-l003-${opt.run}-summary.json`, JSON.stringify({ run: opt.run, url: local ? "local" : U, n: N, cells: rows.summary(), bad,
  pr: { open: pr2?.open, merged: pr2?.merged } }, null, 1));
local?.stop();
