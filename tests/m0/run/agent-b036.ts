// M0-WP10, B0-36: agent refusals with raw tokens, over git, REST, and GraphQL, against graft-m0-agent (or a local
// Host with --local). 100 attempts per cell. Raw rows: results/raw/wp10-b036-<run>.jsonl, summary beside it.
//
//   node run/agent-b036.ts [--local] [--url U] [--n 100] [--k 8] [--run X]

import { writeFileSync, mkdirSync, appendFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { ROOT, Work, api, gql, harness, pool, push, tok, baseUrl, type PushResult } from "./agent-lib.ts";
import { startLocal } from "./agent.ts";
import { BASE, FORK, T1, T2, T3, T4, X1, X2 } from "../src/agent/fixture.ts";

const { values: opt } = parseArgs({ options: { local: { type: "boolean", default: false }, url: { type: "string" }, n: { type: "string", default: "100" },
  k: { type: "string", default: "8" }, only: { type: "string" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const N = Number(opt.n), K = Number(opt.k);
const local = opt.local ? await startLocal() : null;
const U = local ? local.url : baseUrl(opt);
const RAW = `${ROOT}results/raw`;
mkdirSync(RAW, { recursive: true });
const rawPath = `${RAW}/wp10-b036-${opt.run}.jsonl`;
writeFileSync(rawPath, "");

const git = (repo: string) => `${U}/${repo}.git`;
const W = new Work(`b036-${opt.run}`);
await W.init();

interface Attempt { refused: boolean; status: string; text: string; ms: number }
interface Row extends Attempt { cell: string; kind: string; action: string; surface: string; expect: "refused" | "accepted" | "undefined"; i: number; probe: string;
                                rule?: string; actorKind?: string; admit?: string; sources?: unknown }

const gitAttempt = (r: PushResult): Attempt => {
  const bad = r.refs.find((x) => x.flag === "!");
  if (!r.refs.length) return { refused: true, status: /error: (\d{3})|returned error: (\d{3})/.exec(r.err)?.slice(1).find(Boolean) ?? `exit ${r.code}`, text: r.err.slice(-200), ms: r.ms };
  return { refused: !!bad, status: bad ? "ng" : "ok", text: bad ? bad.summary : r.refs.map((x) => x.summary).join(";"), ms: r.ms };
};
const restAttempt = (r: { status: number; body: any; ms: number }): Attempt =>
  ({ refused: r.status >= 400, status: String(r.status), text: typeof r.body === "object" && r.body ? r.body.message ?? JSON.stringify(r.body).slice(0, 200) : String(r.body).slice(0, 200), ms: r.ms });
const gqlAttempt = (r: { status: number; body: any; ms: number }): Attempt => {
  const e = r.body?.errors?.[0];
  return { refused: r.status >= 400 || !!e, status: e ? `${r.status} ${e.type}` : String(r.status), text: e ? e.message : JSON.stringify(r.body?.data ?? r.body).slice(0, 200), ms: r.ms };
};

const rows: Row[] = [];
async function cell(cellName: string, kind: string, action: string, surface: string, expect: Row["expect"], f: (i: number, probe: string) => Promise<Attempt>, n = N, k = K): Promise<void> {
  if (opt.only && !new RegExp(opt.only).test(cellName)) return;
  const t = Date.now();
  const out = await pool(n, k, async (i) => {
    const probe = `${opt.run}-${cellName}-${i}`;
    let a: Attempt;
    try { a = await f(i, probe); } catch (e) { a = { refused: false, status: "driver-error", text: String(e).slice(0, 300), ms: 0 }; }
    return { cell: cellName, kind, action, surface, expect, i, probe, ...a } as Row;
  });
  // Join the host's audit rows for the rule that refused and the actor's class.
  for (let s = 0; s < out.length; s += 500) {
    const audit = await harness(U, "/m0/audit", out.slice(s, s + 500).map((r) => r.probe)) as ({ rule?: string; kind: string; admit: string; ok: boolean; sources?: unknown } | null)[];
    audit.forEach((a, j) => { const r = out[s + j]; if (a) { r.rule = a.ok ? "ok" : a.rule; r.actorKind = a.kind; r.admit = a.admit; r.sources = a.sources; } });
  }
  for (const r of out) appendFileSync(rawPath, JSON.stringify(r) + "\n");
  rows.push(...out);
  const ref = out.filter((r) => r.refused).length;
  const rules = [...new Set(out.map((r) => r.rule ?? "?"))].join(",");
  console.log(`${cellName.padEnd(44)} ${String(ref).padStart(3)}/${out.length} refused  rules=${rules}  ${((Date.now() - t) / 1000).toFixed(1)}s`);
}

// ---- Bed ----
console.log(`B0-36 against ${U}, run ${opt.run}`);
await harness(U, "/m0/setup", {});
const T = { alice: await tok("alice"), bob: await tok("bob"), carol: await tok("carol"), grunAgent: await tok("grun-agent"), grunWs: await tok("grun-ws"),
  grunWide: await tok("grun-wide"), notask: await tok("grun-ws-notask"), done: await tok("grun-ws-done"), t3: await tok("grun-ws-t3"),
  vendor: await tok("vendor"), flagged: await tok("flagged"), mcp: await tok("mcp-bob") };
const VH = { "X-Graftyard-Agent-Session": X1 };
const must = (r: PushResult, what: string) => { if (r.code !== 0) throw new Error(`${what}: ${r.err}`); };

const main = await W.commit(null, { "README.md": "mono\n", "pkg/auth/session.go": "package auth\n", ".github/workflows/ci.yml": "on: push\n", "src/a.txt": "a\n", "AGENTS.md": "rules\n" }, "init\n");
await W.git(["update-ref", "refs/heads/main", main]);
must(await push(W, git(BASE), T.alice, [`${main}:refs/heads/main`]), "seed main");
await harness(U, "/m0/fork-sync", {});
const out = { wf: await W.commit(main, { ".github/workflows/ci.yml": "on: [push, pull_request]\nsecrets: leak\n" }, "outsider workflow change\n"),
              lease: await W.commit(main, { "pkg/auth/session.go": "package auth // bob\n" }, "bob auth change\n"),
              plain: await W.commit(main, { "src/b.txt": "b\n" }, "plain\n"),
              agents: await W.commit(main, { "AGENTS.md": "rules, edited\n" }, "agents edit\n") };
must(await push(W, git(BASE), T.bob, [`${out.wf}:refs/heads/bob/wf`, `${out.lease}:refs/heads/bob/lease`, `${out.plain}:refs/heads/bob/plain`, `${out.agents}:refs/heads/bob/agents`]), "bob refs");
const P1 = `refs/heads/graftyard/${T1}/`;
const grunPr = await W.commit(main, { "src/agent.txt": "1\n" }, "agent work\n");
const grunBase = await W.commit(main, { "src/agent-base.txt": "1\n" }, "agent base\n");
must(await push(W, git(BASE), T.grunWs, [`${grunPr}:${P1}pr`, `${grunBase}:${P1}base`]), "grun refs (control: in-prefix push accepted)");
const vPr = await W.commit(main, { "src/vendor.txt": "1\n" }, "vendor work\n");
const vBase = await W.commit(main, { "src/vendor-base.txt": "1\n" }, "vendor base\n");
must(await push(W, git(BASE), T.vendor, [`${vPr}:refs/heads/claude/pr`, `${vBase}:refs/heads/claude/base`], { headers: VH }), "vendor refs (control)");
const pr1 = await api(U, "POST", `/api/v3/repos/${BASE}/pulls`, T.grunAgent, { head: `graftyard/${T1}/pr`, base: "main", draft: true, title: "agent" }, { "X-Graftyard-Agent-Session": T1 });
const pr2 = await api(U, "POST", `/api/v3/repos/${BASE}/pulls`, T.vendor, { head: "claude/pr", base: "main", draft: true, title: "vendor" }, VH);
if (pr1.status !== 201 || pr2.status !== 201) throw new Error(`PR open: ${JSON.stringify([pr1, pr2])}`);
const forkW = await W.commit(main, { "src/fork.txt": "1\n" }, "fork work\n");
must(await push(W, git(FORK), T.t3, [`${forkW}:refs/heads/graftyard/${T3}/w`]), "fork push before cancel (control)");
const cancel = await api(U, "POST", `/api/graftyard/v1/repos/${BASE}/tasks/${T3}/cancel`, T.alice, {});
if (cancel.status !== 200) throw new Error(`cancel: ${JSON.stringify(cancel)}`);
console.log(`bed: main ${main.slice(0, 7)}, PR #${pr1.body.number} (graftyard-agent, draft), PR #${pr2.body.number} (vendor, draft), task ${T3.slice(0, 8)} cancelled`);

// Per-attempt fresh commits on main, so every push carries its own pack.
const fresh = (tag: string, files: Record<string, string>) => async (i: number) => W.commit(main, Object.fromEntries(Object.entries(files).map(([p, c]) => [p, `${c} ${tag} ${i}\n`])), `${tag} ${i}\n`);

interface Kind { name: string; api: string; push: string; prefix: string; pr: number; prHead: string; base: string; baseTip: string; headers: Record<string, string> }
const kinds: Kind[] = [
  { name: "grun", api: T.grunAgent, push: T.grunWs, prefix: P1, pr: pr1.body.number, prHead: `${P1}pr`, base: `${P1}base`, baseTip: grunBase, headers: {} },
  { name: "grun-wide", api: T.grunWide, push: T.grunWide, prefix: P1, pr: pr1.body.number, prHead: `${P1}pr`, base: `${P1}base`, baseTip: grunBase, headers: {} },
  { name: "vendor", api: T.vendor, push: T.vendor, prefix: "refs/heads/claude/", pr: pr2.body.number, prHead: "refs/heads/claude/pr", base: "refs/heads/claude/base", baseTip: vBase, headers: VH },
];

const R = (path: string) => `/api/v3/repos/${BASE}${path}`;
const restCall = (k: Kind, method: string, path: string, body?: unknown, tokOverride?: string, extra: Record<string, string> = {}) =>
  async (_i: number, probe: string) => restAttempt(await api(U, method, R(path), tokOverride ?? k.api, body, { ...k.headers, ...extra, "x-m0-probe": probe }));
const gqlCall = (k: Kind, m: string, input: (i: number) => unknown, tokOverride?: string) =>
  async (i: number, probe: string) => gqlAttempt(await api(U, "POST", "/api/graphql", tokOverride ?? k.api,
    { query: `mutation($input: ${m[0].toUpperCase()}${m.slice(1)}Input!) { ${m}(input: $input) { clientMutationId } }`, variables: { input: input(i) } }, { ...k.headers, "x-m0-probe": probe }));
const gitPush = (k: Kind, spec: (i: number) => Promise<string[]>, opts: { token?: string; repo?: string; headers?: Record<string, string> } = {}) =>
  async (i: number, probe: string) => gitAttempt(await push(W, git(opts.repo ?? BASE), opts.token ?? k.push, await spec(i), { headers: opts.headers ?? k.headers, probe }));

for (const k of kinds) {
  const c = (a: string, s: string) => `${k.name}/${a}/${s}`;
  const prId = `PR_${k.pr}`;
  await cell(c("approve", "rest"), k.name, "approve", "rest", "refused", restCall(k, "POST", `/pulls/${k.pr}/reviews`, { event: "APPROVE" }));
  await cell(c("approve", "graphql"), k.name, "approve", "graphql", "refused", gqlCall(k, "addPullRequestReview", () => ({ pullRequestId: prId, event: "APPROVE" })));
  await cell(c("merge", "rest"), k.name, "merge", "rest", "refused", restCall(k, "PUT", `/pulls/${k.pr}/merge`, {}));
  await cell(c("merge", "graphql"), k.name, "merge", "graphql", "refused", gqlCall(k, "mergePullRequest", () => ({ pullRequestId: prId })));
  const prTip = k.name === "vendor" ? vPr : grunPr;
  await cell(c("merge", "git"), k.name, "merge (fast-forward main to the PR head)", "git", "refused", gitPush(k, async () => [`${prTip}:refs/heads/main`]));
  await cell(c("ready", "graphql"), k.name, "ready-for-review", "graphql", "refused", gqlCall(k, "markPullRequestReadyForReview", () => ({ pullRequestId: prId })));
  await cell(c("bypass", "rest"), k.name, "bypass (ruleset edit)", "rest", "refused", restCall(k, "PUT", "/rulesets/7", { bypass_actors: [{ actor_type: "Integration" }] }));
  await cell(c("bypass", "graphql"), k.name, "bypass (ruleset edit)", "graphql", "refused", gqlCall(k, "updateRepositoryRuleset", () => ({ repositoryRulesetId: "RRS_7", bypassActors: [] })));
  const nonFf = fresh(`${k.name}-force`, { "src/force.txt": "x" });
  await cell(c("bypass", "git"), k.name, "bypass (force-push past ruleset 8)", "git", "refused", gitPush(k, async (i) => [`+${await nonFf(i)}:${k.base}`]));
  await cell(c("dispatch", "rest"), k.name, "repository dispatch", "rest", "refused", restCall(k, "POST", "/dispatches", { event_type: "x" }));
  await cell(c("dispatch-workflow", "rest"), k.name, "workflow dispatch", "rest", "refused", restCall(k, "POST", "/actions/workflows/ci.yml/dispatches", { ref: "main" }));
  await cell(c("rerun", "rest"), k.name, "workflow re-run", "rest", "refused", restCall(k, "POST", "/actions/runs/1/rerun", {}));
  await cell(c("rerun", "graphql"), k.name, "workflow re-run", "graphql", "refused", gqlCall(k, "rerequestCheckSuite", () => ({ checkSuiteId: "CS_1", repositoryId: `R_${BASE}` })));
  await cell(c("release", "rest"), k.name, "release", "rest", "refused", restCall(k, "POST", "/releases", { tag_name: "v1" }));
  await cell(c("release", "graphql"), k.name, "release (tag ref)", "graphql", "refused", gqlCall(k, "createRef", (i) => ({ repositoryId: `R_${BASE}`, name: `refs/tags/${k.name}-g${i}`, oid: main }), k.push));
  await cell(c("release", "git"), k.name, "release (tag push)", "git", "refused", gitPush(k, async (i) => [`${main}:refs/tags/${k.name}-v${i}`]));
  await cell(c("label", "rest"), k.name, "label outside the allowlist", "rest", "refused", restCall(k, "POST", `/issues/${k.pr}/labels`, { labels: ["bug"] }));
  await cell(c("label", "graphql"), k.name, "label outside the allowlist", "graphql", "refused", gqlCall(k, "addLabelsToLabelable", () => ({ labelableId: prId, labelIds: ["LA_bug"] })));
  await cell(c("status", "rest"), k.name, "graftyard/ status", "rest", "refused", restCall(k, "POST", `/statuses/${prTip}`, { state: "success", context: "graftyard/provenance" }));
  await cell(c("status", "graphql"), k.name, "graftyard/ check run", "graphql", "refused", gqlCall(k, "createCheckRun", () => ({ repositoryId: `R_${BASE}`, headSha: prTip, name: "graftyard/overlap" })));
  await cell(c("agent-gate", "rest"), k.name, "post graftyard/agent-gate", "rest", "refused", restCall(k, "POST", `/statuses/${prTip}`, { state: "success", context: "graftyard/agent-gate" }));
  await cell(c("agent-gate", "graphql"), k.name, "post graftyard/agent-gate", "graphql", "refused", gqlCall(k, "createCheckRun", () => ({ repositoryId: `R_${BASE}`, headSha: prTip, name: "graftyard/agent-gate" })));
  // Pushes: outside the prefix.
  const outside = fresh(`${k.name}-out`, { "src/out.txt": "x" });
  await cell(c("outside-prefix", "git"), k.name, "push outside its prefix", "git", "refused", gitPush(k, async (i) => [`${await outside(i)}:refs/heads/feature/${k.name}-${i}`]));
  await cell(c("outside-prefix", "rest"), k.name, "push outside its prefix", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), k.push, { ref: `refs/heads/feature/${k.name}-r${i}`, sha: out.plain }, { ...k.headers, "x-m0-probe": probe })));
  await cell(c("outside-prefix", "graphql"), k.name, "push outside its prefix", "graphql", "refused", gqlCall(k, "createRef", (i) => ({ repositoryId: `R_${BASE}`, name: `refs/heads/feature/${k.name}-g${i}`, oid: out.plain }), k.push));
  // Pushes: no live task.
    // The design's refusal list has "a Graftyard-run push with no live AgentTask" only; a confined vendor App is bound by
    // `confine`, and vendors have no workspace fork. These cells run the design as written and record what it does.
  if (k.name === "vendor") {
    const nt = fresh("vendor-nt", { "src/nt.txt": "x" });
    await cell(c("no-live-task", "git"), k.name, "push with no live task (no session)", "git", "undefined", gitPush(k, async (i) => [`${await nt(i)}:refs/heads/claude/nt-${i}`], { headers: {} }));
    await cell(c("no-live-task", "rest"), k.name, "push with no live task (no session)", "rest", "undefined", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), k.push, { ref: `refs/heads/claude/ntr-${i}`, sha: out.plain }, { "x-m0-probe": probe })));
    await cell(c("no-live-task", "graphql"), k.name, "push with no live task (no session)", "graphql", "undefined", async (i, probe) => gqlAttempt(await api(U, "POST", "/api/graphql", k.push,
      { query: "mutation($input: CreateRefInput!) { createRef(input: $input) { clientMutationId } }", variables: { input: { repositoryId: `R_${BASE}`, name: `refs/heads/claude/ntg-${i}`, oid: out.plain } } }, { "x-m0-probe": probe })));
    const ft = fresh("vendor-cancelled", { "src/ct.txt": "x" });
    await cell(c("cancelled-task", "git"), k.name, "push after its External task is cancelled", "git", "undefined", gitPush(k, async (i) => [`${await ft(i)}:refs/heads/claude/ct-${i}`], { headers: { "X-Graftyard-Agent-Session": X2 } }));
  } else {
    const nt = fresh(`${k.name}-nt`, { "src/nt.txt": "x" });
    const tokNo = k.name === "grun" ? T.notask : T.notask;
    await cell(c("no-live-task", "git"), k.name, "push with no task claim", "git", "refused", gitPush(k, async (i) => [`${await nt(i)}:${P1}nt-${i}`], { token: tokNo }));
    await cell(c("no-live-task", "rest"), k.name, "push with no task claim", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), tokNo, { ref: `${P1}ntr-${i}`, sha: out.plain }, { "x-m0-probe": probe })));
    await cell(c("no-live-task", "graphql"), k.name, "push with no task claim", "graphql", "refused", gqlCall(k, "createRef", (i) => ({ repositoryId: `R_${BASE}`, name: `${P1}ntg-${i}`, oid: out.plain }), tokNo));
    const P4 = `refs/heads/graftyard/${T4}/`;
    await cell(c("completed-task", "git"), k.name, "push for a completed task", "git", "refused", gitPush(k, async (i) => [`${await nt(i + 1000)}:${P4}d-${i}`], { token: T.done }));
    // Fork push after its task is cancelled (fork mode; the fork group reads the task from the base).
    const fk = fresh(`${k.name}-fork`, { "src/fork.txt": "x" });
    const P3 = `refs/heads/graftyard/${T3}/`;
    await cell(c("fork-cancelled", "git"), k.name, "fork push after its task is cancelled", "git", "refused", gitPush(k, async (i) => [`${await fk(i)}:${P3}c-${i}`], { token: T.t3, repo: FORK }));
    await cell(c("fork-cancelled", "rest"), k.name, "fork push after its task is cancelled", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", `/api/v3/repos/${FORK}/git/refs`, T.t3, { ref: `${P3}r-${i}`, sha: main }, { "x-m0-probe": probe })));
    await cell(c("fork-cancelled", "graphql"), k.name, "fork push after its task is cancelled", "graphql", "refused", gqlCall(k, "createRef", (i) => ({ repositoryId: `R_${FORK}`, name: `${P3}g-${i}`, oid: main }), T.t3));
  }
  // Exclusive scope of another task (T2 holds pkg/auth/** Exclusive), AgentsExclusive on.
  const lz = fresh(`${k.name}-lease`, { "pkg/auth/session.go": "package auth //" });
  await cell(c("lease", "git"), k.name, "push into another task's Exclusive scope", "git", "refused", gitPush(k, async (i) => [`${await lz(i)}:${k.prefix}l-${i}`]));
  await cell(c("lease", "rest"), k.name, "REST ref write into another task's Exclusive scope", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), k.push, { ref: `${k.prefix}lr-${i}`, sha: out.lease }, { ...k.headers, "x-m0-probe": probe })));
  await cell(c("lease", "graphql"), k.name, "commit into another task's Exclusive scope", "graphql", "refused", gqlCall(k, "createCommitOnBranch", (i) => ({ branch: { repositoryNameWithOwner: BASE, branchName: k.prHead.replace("refs/heads/", "") },
    expectedHeadOid: prTip, message: { headline: `lease ${i}` }, fileChanges: { additions: [{ path: "pkg/auth/token.go", contents: Buffer.from(`package auth // ${i}\n`).toString("base64") }] } }), k.push));
  // Protected paths.
  const pz = fresh(`${k.name}-prot`, { ".github/workflows/ci.yml": "on: push #" });
  await cell(c("protected", "git"), k.name, "change under the protected list", "git", "refused", gitPush(k, async (i) => [`${await pz(i)}:${k.prefix}p-${i}`]));
  await cell(c("protected", "rest"), k.name, "change under the protected list (AGENTS.md)", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), k.push, { ref: `${k.prefix}pr-${i}`, sha: out.agents }, { ...k.headers, "x-m0-probe": probe })));
  await cell(c("protected", "graphql"), k.name, "change under the protected list", "graphql", "refused", gqlCall(k, "createCommitOnBranch", (i) => ({ branch: { repositoryNameWithOwner: BASE, branchName: k.prHead.replace("refs/heads/", "") },
    expectedHeadOid: prTip, message: { headline: `prot ${i}` }, fileChanges: { additions: [{ path: ".github/workflows/agent.yml", contents: Buffer.from(`on: push # ${i}\n`).toString("base64") }] } }), k.push));
  // An outsider's existing commit that changes a workflow, pushed under its own ref (no new objects).
  await cell(c("outsider-workflow", "git"), k.name, "outsider commit changing a workflow, under its ref", "git", "refused", gitPush(k, async (i) => [`${out.wf}:${k.prefix}o-${i}`]));
  await cell(c("outsider-workflow", "rest"), k.name, "outsider commit changing a workflow, under its ref", "rest", "refused", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), k.push, { ref: `${k.prefix}or-${i}`, sha: out.wf }, { ...k.headers, "x-m0-probe": probe })));
}

// ---- Push after an approval, then merge (the gate is head-bound) ----
const readyTok = T.alice;
for (const k of kinds.filter((x) => x.name !== "grun-wide" && (!opt.only || new RegExp(opt.only).test(`${x.name}/push-after-approval`)))) {
  const rr = await gqlAttempt(await api(U, "POST", "/api/graphql", readyTok, { query: "mutation($input: MarkPullRequestReadyForReviewInput!) { markPullRequestReadyForReview(input: $input) { clientMutationId } }", variables: { input: { pullRequestId: `PR_${k.pr}` } } }));
  if (rr.refused) throw new Error(`alice ready: ${rr.text}`);
  let tip = k.name === "vendor" ? vPr : grunPr;
  const sq: Row[] = [];
  for (let i = 0; i < N; i++) {
    const probe = `${opt.run}-${k.name}/approve-push-merge-${i}`;
    const ap = await api(U, "POST", R(`/pulls/${k.pr}/reviews`), T.bob, { event: "APPROVE" }, { "accept": "application/vnd.graftyard.agent+json" });
    if (!ap.body?.graftyard_gate) throw new Error(`bob approval wrote no gate: ${JSON.stringify(ap)}`);
    const next = await W.commit(tip, { "src/after-approval.txt": `${i}\n` }, `after approval ${i}\n`);
    const pr = await push(W, git(BASE), k.push, [`${next}:${k.prHead}`], { headers: k.headers });
    if (pr.code !== 0) throw new Error(`agent push after approval: ${pr.err}`);
    tip = next;
    const hm = restAttempt(await api(U, "PUT", R(`/pulls/${k.pr}/merge`), T.carol, {}, { "x-m0-probe": `${probe}-human` }));
    const am = restAttempt(await api(U, "PUT", R(`/pulls/${k.pr}/merge`), k.api, {}, { ...k.headers, "x-m0-probe": `${probe}-agent` }));
    sq.push({ cell: `${k.name}/push-after-approval/human-merge`, kind: k.name, action: "human merge after an agent push that followed approval", surface: "rest", expect: "refused", i, probe: `${probe}-human`, ...hm });
    sq.push({ cell: `${k.name}/push-after-approval/agent-merge`, kind: k.name, action: "agent merge after its push that followed approval", surface: "rest", expect: "refused", i, probe: `${probe}-agent`, ...am });
  }
  const audit = await harness(U, "/m0/audit", sq.map((r) => r.probe)) as ({ rule?: string; ok: boolean } | null)[];
  audit.forEach((a, j) => { if (a) sq[j].rule = a.ok ? "ok" : a.rule; });
  for (const r of sq) appendFileSync(rawPath, JSON.stringify(r) + "\n");
  rows.push(...sq);
  for (const nm of ["human-merge", "agent-merge"]) {
    const s = sq.filter((r) => r.cell.endsWith(nm));
    console.log(`${(`${k.name}/push-after-approval/${nm}`).padEnd(44)} ${String(s.filter((r) => r.refused).length).padStart(3)}/${s.length} refused  rules=${[...new Set(s.map((r) => r.rule))].join(",")}`);
  }
  // Control: a fresh approval on the current head lets a human merge.
  await api(U, "POST", R(`/pulls/${k.pr}/reviews`), T.bob, { event: "APPROVE" });
  const ok = await api(U, "PUT", R(`/pulls/${k.pr}/merge`), T.carol, {}, { "x-m0-probe": `${opt.run}-${k.name}/control-merge` });
  rows.push({ cell: `${k.name}/control-merge`, kind: k.name, action: "human merge with the gate on the current head", surface: "rest", expect: "accepted", i: 0, probe: "", ...restAttempt(ok) });
  console.log(`${k.name}/control-merge: ${ok.status} ${JSON.stringify(ok.body).slice(0, 80)}`);
}

// ---- The other three kinds into the same Exclusive scope, and the Co-authored-by push ----
const others: { name: string; token: string; prefix: string; headers: Record<string, string> }[] = [
  { name: "flagged-no-confine", token: T.flagged, prefix: "refs/heads/cursor/", headers: {} },
  { name: "graftyard-mcp", token: T.mcp, prefix: "refs/heads/mcp/", headers: {} },
  { name: "user+session-header", token: T.bob, prefix: "refs/heads/bob/", headers: { "X-Graftyard-Agent-Session": T2 } },
];
for (const o of others) {
  const lz = fresh(`${o.name}-lease`, { "pkg/auth/session.go": "package auth //" });
  await cell(`${o.name}/lease/git`, o.name, "push into another task's Exclusive scope", "git", "accepted", async (i, probe) => gitAttempt(await push(W, git(BASE), o.token, [`${await lz(i)}:${o.prefix}l-${i}`], { headers: o.headers, probe })));
  await cell(`${o.name}/lease/rest`, o.name, "REST ref write into another task's Exclusive scope", "rest", "accepted", async (i, probe) => restAttempt(await api(U, "POST", R("/git/refs"), o.token, { ref: `${o.prefix}lr-${i}`, sha: out.lease }, { ...o.headers, "x-m0-probe": probe })));
}
const co = fresh("co-authored", { "src/co.txt": "x" });
await cell("user/co-authored-by/git", "user", "user-token push whose only trailer is Co-authored-by", "git", "accepted", async (i, probe) => {
  const c = await W.commit(main, { "src/co.txt": `${i}\n` }, `fix ${i}\n\nCo-authored-by: Claude <noreply@anthropic.com>\n`);
  void co;
  return gitAttempt(await push(W, git(BASE), T.bob, [`${c}:refs/heads/bob/co-${i}`], { probe }));
});

// ---- Summary ----
const byCell = new Map<string, Row[]>();
for (const r of rows) byCell.set(r.cell, [...(byCell.get(r.cell) ?? []), r]);
const summary = [...byCell].map(([cellName, rs]) => ({ cell: cellName, kind: rs[0].kind, action: rs[0].action, surface: rs[0].surface, expect: rs[0].expect, n: rs.length,
  refused: rs.filter((r) => r.refused).length, rules: Object.fromEntries([...new Set(rs.map((r) => r.rule ?? "?"))].map((k) => [k, rs.filter((r) => (r.rule ?? "?") === k).length])),
  statuses: [...new Set(rs.map((r) => r.status))].slice(0, 4), sample: rs.find((r) => r.refused)?.text ?? rs[0].text,
  admit: [...new Set(rs.map((r) => r.admit).filter(Boolean))], actorKind: [...new Set(rs.map((r) => r.actorKind).filter(Boolean))],
  sources: rs[0].sources, p50ms: rs.map((r) => r.ms).sort((a, b) => a - b)[Math.floor(rs.length / 2)] }));
const state = await harness(U, "/m0/state");
writeFileSync(`${RAW}/wp10-b036-${opt.run}-summary.json`, JSON.stringify({ run: opt.run, url: local ? "local" : U, n: N, cells: summary, counts: (state as any).counts }, null, 1));
const bad = summary.filter((s) => s.expect === "undefined" ? false : s.expect === "refused" ? s.refused !== s.n
  : s.refused !== 0 || (s.rules["Leased"] ?? 0) > 0 || (s.cell.startsWith("user/co-authored-by") && (s.admit.length !== 1 || s.admit[0] !== "Human")));
console.log(`\n${summary.length} cells, ${rows.length} attempts; cells off their expectation: ${bad.length}`);
for (const b of bad) console.log(`  ${b.cell}: ${b.refused}/${b.n} refused, rules ${JSON.stringify(b.rules)}, ${b.sample}`);
// The fold's refs against git's refs on the container's disk (a restart mid-run would show here).
const check = { ...(await harness(U, "/m0/check")), boots: (await harness(U, "/m0/state")).boots ?? 0 };
console.log(`fold vs git refs: ${JSON.stringify(check)}`);
writeFileSync(`${RAW}/${"wp10-b036"}-${opt.run}-check.json`, JSON.stringify(check, null, 1));
local?.stop();
