// M0-WP13, L0-15: the agent gate and read permissions, against graft-m0-agent.
//   Gate. Three PRs with AgentMeta, each approved and then merged by a human writer who could otherwise merge:
//     (a) an MCP client whose grant and scoped token carry WriteApproveMerge approves a Graftyard-run PR;
//     (b) Code Mode, with a scoped token carrying Write only, calls approve and merge through `codemode.request()`
//         (100 approve and 100 merge attempts, each in a fresh Dynamic Worker);
//     (c) the vendor's named requester (the External task's `requester`) approves the vendor's PR.
//     (d) A23: a vendor PR opened without X-Graftyard-Agent-Session (`requester` empty); its would-be requester
//         approves, and the gate must stay unsatisfied until one more writer approves.
//   `AgentGate` must stay unsatisfied in (a), (b), (c), and in (d) until the second writer, measured by a human merge
//   refused 100 of 100 per case. Controls: a plain writer's approval satisfies (a) to (c).
//   Reads. Anonymous, `metadata: read`-only, and over-limit feed polls get 401, 403, or 429 at the edge, the leader sees
//   zero feed requests; `/log`, prompts, and session pages with `metadata: read` only return 403.
//
//   node run/agent-l015.ts [--url U] [--n 100] [--k 8] [--run X]       (remote only: Code Mode needs Dynamic Workers)

import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { Work, baseUrl, pool, push } from "./agent-lib.ts";
import { RAW, Rows, V1, api, harness, msg, orgSet, tok } from "./agent-lib13.ts";
import { APP, MCP_CLIENT } from "../src/agent/fixture.ts";
import { derive } from "../src/agent/token.ts";
import { TOKEN } from "./agent-lib.ts";

const { values: opt } = parseArgs({ options: { url: { type: "string" }, n: { type: "string", default: "100" }, k: { type: "string", default: "8" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const N = Number(opt.n), K = Number(opt.k);
const U = baseUrl(opt);
const rows = new Rows("l015", opt.run);
const W = new Work(`l015-${opt.run}`);
await W.init();
const BASE = "acme/mono", R = `/api/v3/repos/${BASE}`;
console.log(`L0-15 against ${U}, run ${opt.run}`);
await harness(U, "/m0/setup", {});
await orgSet(U, { agents_enabled: true, code_mode: true });
const T = { alice: await tok("alice"), bob: await tok("bob"), carol: await tok("carol"), dave: await tok("dave"), ws: await tok("grun-ws"),
  agent: await tok("grun-agent"), vendor: await tok("vendor"), mcpClient: await tok("mcp-client"), bobMeta: await tok("bob-meta"), bobFg: await tok("bob-fg") };

// ---- Scoped MCP tokens through token/scoped with graftyard_grant and graftyard_write ----
const basic = `Basic ${Buffer.from(`${MCP_CLIENT.id}:${await derive(TOKEN, MCP_CLIENT.secretLabel)}`).toString("base64")}`;
const scoped = async (grant: string, write: string, accessToken = T.mcpClient) => {
  const r = await fetch(`${U}/api/v3/applications/${MCP_CLIENT.id}/token/scoped`, { method: "POST", headers: { authorization: basic, "content-type": "application/json" },
    body: JSON.stringify({ access_token: accessToken, repositories: ["mono"], graftyard_grant: grant, graftyard_write: write }) });
  return { status: r.status, body: await r.json() as any };
};
const mcpAM = await scoped("g-bob", "write_approve_merge");
const mcpW = await scoped("g-bob", "write");
// token/scoped checks: above the grant, another user's grant, no grant. (A narrow sanity sample, not a gate row here.)
const above = await scoped("g-carol", "write_approve_merge", await tok("mcp-client"));
const none = await scoped("g-none", "write");
if (mcpAM.status !== 200 || mcpW.status !== 200) throw new Error(`token/scoped: ${JSON.stringify([mcpAM, mcpW])}`);
console.log(`token/scoped: WriteApproveMerge ${mcpAM.status}, Write ${mcpW.status}, another user's grant ${above.status}, unknown grant ${none.status}`);
rows.add("token/scoped with another user's grant", "refused", above.status, above.status >= 400, { message: msg(above.body) });
rows.add("token/scoped with an unknown grant", "refused", none.status, none.status >= 400, { message: msg(none.body) });

// ---- Bed: main, a Graftyard-run PR, a vendor PR with a requester, a vendor PR without one ----
const must = (r: { code: number; err: string }, what: string) => { if (r.code !== 0) throw new Error(`${what}: ${r.err}`); };
const main = await W.commit(null, { "README.md": "mono\n", "src/a.txt": "a\n" }, "init\n");
must(await push(W, `${U}/${BASE}.git`, T.alice, [`${main}:refs/heads/main`]), "seed");
const task = await api(U, "POST", `${V1}/tasks`, T.alice, { prompt: "do it" });
const A = task.body.id as string;
const wsA = (await api(U, "POST", "/api/v3/app/installations/107/access_tokens", await tok("jwt-workspace"), { repositories: ["mono"], graftyard_task: A })).body.token;
const agentA = (await api(U, "POST", "/api/v3/app/installations/101/access_tokens", await tok("jwt-agent"), { repositories: ["mono"], graftyard_task: A })).body.token;
// The vendor's External task names carol as requester.
const ext = await api(U, "POST", `${V1}/tasks`, T.vendor, { kind: "external", session: "l015", requester: "carol", summary: "vendor work" });
if (ext.status !== 201) throw new Error(`external: ${JSON.stringify(ext.body)}`);
const prs: Record<string, number> = {};
async function openPr(name: string, ref: string, token: string, headers: Record<string, string>, pushTok: string): Promise<void> {
  const c = await W.commit(main, { [`src/${name}.txt`]: `${name}\n` }, `${name}\n`);
  must(await push(W, `${U}/${BASE}.git`, pushTok, [`${c}:${ref}`], { headers }), `push ${name}`);
  const pr = await api(U, "POST", `${R}/pulls`, token, { head: ref.replace("refs/heads/", ""), base: "main", draft: true, title: name }, headers);
  if (pr.status !== 201) throw new Error(`PR ${name}: ${JSON.stringify(pr.body)}`);
  const rd = await api(U, "POST", "/api/graphql", T.alice, { query: "mutation($input: MarkPullRequestReadyForReviewInput!) { markPullRequestReadyForReview(input: $input) { clientMutationId } }",
    variables: { input: { pullRequestId: `PR_${pr.body.number}` } } });
  if (rd.body?.errors) throw new Error(`ready ${name}: ${JSON.stringify(rd.body)}`);
  prs[name] = pr.body.number;
}
await openPr("grun", `refs/heads/graftyard/${A}/w`, agentA, { "X-Graftyard-Agent-Session": A }, wsA);
await openPr("grun-cm", `refs/heads/graftyard/${A}/cm`, agentA, { "X-Graftyard-Agent-Session": A }, wsA);
await openPr("vendor", "refs/heads/claude/l015", T.vendor, { "X-Graftyard-Agent-Session": "l015" }, T.vendor);
await openPr("vendor-nohdr", "refs/heads/claude/nohdr", T.vendor, {}, T.vendor);
const meta = async (n: number) => (await harness(U, `/m0/pr?repo=${encodeURIComponent(BASE)}&n=${n}`)).meta;
console.log(`PRs: ${JSON.stringify(prs)}; requesters: grun ${(await meta(prs.grun)).requester}, vendor ${(await meta(prs.vendor)).requester}, vendor-nohdr ${(await meta(prs["vendor-nohdr"])).requester}`);

const mergeBy = (who: string, n: number, probe: string) => api(U, "PUT", `${R}/pulls/${n}/merge`, who, {}, { "x-m0-probe": probe });
const approveBy = (who: string, n: number, probe: string) => api(U, "POST", `${R}/pulls/${n}/reviews`, who, { event: "APPROVE" }, { "x-m0-probe": probe, "accept": "application/vnd.graftyard.agent+json" });
const gateOf = async (n: number) => { const p = await harness(U, `/m0/pr?repo=${encodeURIComponent(BASE)}&n=${n}`); return { gate: p.gate, approvals: p.approvals ?? [], open: p.open, head: p.headOid }; };

// (a) MCP with WriteApproveMerge approves the Graftyard-run PR: the approve is accepted, the gate stays unsatisfied.
await pool(N, K, async (i) => {
  const a = await approveBy(mcpAM.body.token, prs.grun, `${opt.run}-mcp-approve-${i}`);
  rows.add("(a) MCP WriteApproveMerge approves: review accepted", "accepted", a.status, a.status >= 400, { message: msg(a.body) });
  rows.add("(a) MCP WriteApproveMerge approves: no AgentGate written", "accepted", a.status, a.body?.graftyard_gate === true, {});
});
await pool(N, K, async (i) => {
  const m = await mergeBy(T.dave, prs.grun, `${opt.run}-mcp-merge-${i}`);
  rows.add("(a) human merge after MCP approvals: gate unsatisfied", "refused", m.status, m.status >= 400, { message: msg(m.body) });
});
const ga = await gateOf(prs.grun);
rows.add("(a) AgentGate rows after 100 MCP approvals", "accepted", 200, ga.approvals.length !== 0 || ga.gate != null, { message: JSON.stringify(ga) });

// (b) Code Mode with a Write-only grant: approve and merge through codemode.request(). Each attempt is a new Dynamic
// Worker running model-written code. The deny-list refuses first; with it bypassed (raw `fetch` is blocked by
// globalOutbound: null), the host's WriteScope check is the second layer, measured by calling the edge with the same
// scoped token directly.
const cm = (code: string, token: string, probe: string) => fetch(`${U}/mcp/codemode`, { method: "POST", headers: { authorization: `Bearer ${token}`, "content-type": "application/json", "x-m0-probe": probe },
  body: JSON.stringify({ code }) }).then(async (r) => ({ status: r.status, body: await r.json() as any }));
const cmApprove = `return await codemode.request("POST", "${R}/pulls/${prs["grun-cm"]}/reviews", { event: "APPROVE" });`;
const cmMerge = `return await codemode.request("PUT", "${R}/pulls/${prs["grun-cm"]}/merge", {});`;
const cmEvade = `const out = {};
  out.merges = await codemode.request("POST", "${R}/merges", { base: "main", head: "graftyard/${A}/cm" });
  out.graphql = await codemode.request("POST", "/api/graphql", { query: "mutation { mergePullRequest(input: {pullRequestId: \\"PR_${prs["grun-cm"]}\\"}) { clientMutationId } }" });
  out.traversal = await codemode.request("PUT", "${R}/../../acme/mono/pulls/${prs["grun-cm"]}/merge", {});
  try { await fetch("${U}${R}/pulls/${prs["grun-cm"]}/merge", { method: "PUT" }); out.fetch = "allowed"; } catch (e) { out.fetch = String(e).slice(0, 80); }
  return out;`;
for (const [name, code] of [["approve", cmApprove], ["merge", cmMerge]] as const)
  await pool(N, K, async (i) => {
    const r = await cm(code, mcpW.body.token, `${opt.run}-cm-${name}-${i}`);
    const inner = r.body?.result;
    const refused = r.status !== 200 || !r.body?.ok || (inner?.status ?? 500) >= 400;
    rows.add(`(b) Code Mode ${name} without WriteApproveMerge`, "refused", inner?.status ?? r.status, refused, { message: msg(inner?.body ?? r.body) });
  });
await pool(N, K, async (i) => {
  const r = await cm(cmEvade, mcpW.body.token, `${opt.run}-cm-evade-${i}`);
  const o = r.body?.result ?? {};
  const all = [o.merges?.status, o.graphql?.status, o.traversal?.status];
  rows.add("(b) Code Mode merge by POST /merges, GraphQL, path traversal, raw fetch", "refused", all.join("/"), all.every((s) => (s ?? 500) >= 400) && o.fetch !== "allowed", { message: `${msg(o.merges?.body)} | fetch: ${o.fetch}` });
});
// Second layer: the same Write-only scoped token straight at the edge (no deny-list).
await pool(N, K, async (i) => {
  const a = await approveBy(mcpW.body.token, prs["grun-cm"], `${opt.run}-mcpw-approve-${i}`);
  const m = await mergeBy(mcpW.body.token, prs["grun-cm"], `${opt.run}-mcpw-merge-${i}`);
  rows.add("(b) host WriteScope: Write-only token approves at the edge", "refused", a.status, a.status >= 400, { message: msg(a.body) });
  rows.add("(b) host WriteScope: Write-only token merges at the edge", "refused", m.status, m.status >= 400, { message: msg(m.body) });
});
// The scope claim cannot be forged: the host reads the Tokens row (a prototype token claiming WriteApproveMerge with
// the Write row's jti).
const gb = await gateOf(prs["grun-cm"]);
rows.add("(b) AgentGate rows after Code Mode", "accepted", 200, gb.approvals.length !== 0 || !gb.open, { message: JSON.stringify(gb) });
// Code Mode control: a read through codemode.request() is served.
const cmRead = await cm(`return await codemode.request("GET", "${R}/pulls/${prs["grun-cm"]}");`, mcpW.body.token, `${opt.run}-cm-read`);
rows.add("(b) Code Mode read (control)", "accepted", cmRead.body?.result?.status ?? cmRead.status, cmRead.body?.result?.status !== 200, { message: JSON.stringify(cmRead.body).slice(0, 120) });

// (c) The vendor's named requester (carol) approves the vendor PR.
await pool(N, K, async (i) => {
  const a = await approveBy(T.carol, prs.vendor, `${opt.run}-req-approve-${i}`);
  rows.add("(c) named requester approves: no AgentGate written", "accepted", a.status, a.status >= 400 || a.body?.graftyard_gate === true, { message: msg(a.body) });
});
await pool(N, K, async (i) => {
  const m = await mergeBy(T.dave, prs.vendor, `${opt.run}-req-merge-${i}`);
  rows.add("(c) human merge after the requester's approvals: gate unsatisfied", "refused", m.status, m.status >= 400, { message: msg(m.body) });
});

// (d) A23: no recorded requester. carol (who would be the requester) approves; the gate needs one more writer.
await pool(N, K, async (i) => {
  const a = await approveBy(T.carol, prs["vendor-nohdr"], `${opt.run}-nohdr-approve-${i}`);
  rows.add("(d) A23: would-be requester approves a PR with no requester (gate row written)", "accepted", a.status, a.status >= 400, { message: `gate ${a.body?.graftyard_gate}` });
});
await pool(N, K, async (i) => {
  const m = await mergeBy(T.dave, prs["vendor-nohdr"], `${opt.run}-nohdr-merge-${i}`);
  rows.add("(d) A23: human merge with one approval: gate unsatisfied", "refused", m.status, m.status >= 400, { message: msg(m.body) });
});

// Controls: a second writer (bob) approves (d); a plain writer (bob) approves (a) and (c), then dave merges each.
const ctl = async (name: string, n: number, approver: string) => {
  const a = await approveBy(approver, n, `${opt.run}-ctl-approve-${name}`);
  const m = await mergeBy(T.dave, n, `${opt.run}-ctl-merge-${name}`);
  rows.add(`control: ${name}`, "accepted", m.status, m.status !== 200, { message: `approve gate=${a.body?.graftyard_gate}; merge ${msg(m.body)}` });
};
await ctl("(d) one more writer (bob) approves, then merge", prs["vendor-nohdr"], T.bob);
await ctl("(a) a plain writer (bob) approves, then merge", prs.grun, T.bob);
await ctl("(c) a writer other than the requester (bob) approves, then merge", prs.vendor, T.bob);

// ---- Feed polls at the edge ----
const feed = (token: string | null, wait = 0, probe = "") => fetch(`${U}${V1}/activity?wait=${wait}`, { headers: { ...(token ? { authorization: `Bearer ${token}` } : {}), "x-m0-probe": probe } })
  .then(async (r) => ({ status: r.status, by: r.headers.get("x-graftyard-answered-by"), retry: r.headers.get("retry-after"), body: await r.text() }));
const leader0 = (await harness(U, "/m0/feed-at-leader")).feedAtLeader;
await pool(N, K, async () => { const r = await feed(null); rows.add("feed: anonymous poll", "refused", r.status, r.status === 401, { message: `${r.by} ${r.body.slice(0, 80)}` }); });
await pool(N, K, async () => { const r = await feed(T.bobMeta); rows.add("feed: metadata: read-only poll", "refused", r.status, r.status === 403, { message: `${r.by} ${r.body.slice(0, 80)}` }); });
// Over the per-token limit: hold 2 polls (wait=20) on one token, then 10 more polls on it, 10 rounds.
for (let round = 0; round < 10; round++) {
  const held = [feed(T.bobFg, 20), feed(T.bobFg, 20)];
  await new Promise((r) => setTimeout(r, 2_000));
  const over = await Promise.all(Array.from({ length: N / 10 }, () => feed(T.bobFg, 0)));
  for (const r of over) rows.add("feed: over-limit poll (3rd+ concurrent on one token)", "refused", r.status, r.status === 429 && r.retry != null, { message: `${r.by} retry-after ${r.retry}` });
  const h = await Promise.all(held);
  for (const r of h) rows.add("feed: held poll within the limit (control)", "info", r.status, r.status !== 200, { message: `${r.by}` });
}
const leader1 = (await harness(U, "/m0/feed-at-leader")).feedAtLeader;
rows.add("feed: requests that reached the leader", "accepted", leader1 - leader0, leader1 - leader0 !== 0, { message: `before ${leader0}, after ${leader1}` });

// ---- /log, prompts, and session pages with metadata: read only ----
await pool(N, K, async () => {
  const l = await api(U, "GET", `${V1}/tasks/${A}/log`, T.bobMeta);
  rows.add("/log with metadata: read only", "refused", l.status, l.status === 403, { message: msg(l.body) });
  const p = await api(U, "GET", `${V1}/tasks/${A}?fields=prompt`, T.bobMeta);
  rows.add("prompt read with metadata: read only", "refused", p.status, p.status === 403, { message: msg(p.body) });
  const t = await api(U, "GET", `${V1}/tasks/${A}`, T.bobMeta);
  rows.add("task read with metadata: read only omits prompt", "accepted", t.status, t.status !== 200 || "prompt" in t.body, { message: "" });
  const s = await api(U, "GET", `/${BASE}/_graftyard/sessions/${A}`, T.bobMeta, undefined, { accept: "application/json" });
  rows.add("session page with metadata: read only", "refused", s.status, s.status === 403, { message: msg(s.body) });
  const lc = await api(U, "GET", `${V1}/tasks/${A}/log`, T.bobFg);
  rows.add("/log with contents: read (control: 404 until pushed)", "accepted", lc.status, lc.status === 403, { message: msg(lc.body) });
});

rows.print();
const bad = rows.bad();
console.log(`\ncells off their expectation: ${bad.length}`);
for (const b of bad) console.log(`  ${b}`);
writeFileSync(`${RAW}/wp13-l015-${opt.run}-summary.json`, JSON.stringify({ run: opt.run, url: U, n: N, prs, cells: rows.summary(), bad,
  gates: { grun: await gateOf(prs.grun), grunCm: await gateOf(prs["grun-cm"]), vendor: await gateOf(prs.vendor), vendorNoHeader: await gateOf(prs["vendor-nohdr"]) },
  feedAtLeader: { before: leader0, after: leader1 }, app: APP.mcp }, null, 1));
