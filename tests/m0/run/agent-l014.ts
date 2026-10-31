// M0-WP13, L0-14: task rules against graft-m0-agent. With `agents_enabled` off, then on: Graftyard-run creates from an
// installation token, an AgentApp's user token, an MCP token, a demoted user, and a read-only user; a controller create
// from an issue assigned to another bot; 101 open tasks for one requester; PATCH of a cancelled task to in_progress; a
// token request naming another App's task or a repository outside the token's list; a body that sets `prefix` (and
// `session`). 100 attempts per bad case. Every create and cancel must reach the controller's webhook (graft-wp13-hooks)
// within 5 s, and no other App's.
//
//   node run/agent-l014.ts [--local] [--url U] [--n 100] [--k 8] [--run X]

import { writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { baseUrl, pool } from "./agent-lib.ts";
import { startLocal } from "./agent.ts";
import { Pace, RAW, Rows, V1, api, harness, hookLog, hookReset, msg, orgSet, tok } from "./agent-lib13.ts";
import { T1, X1 } from "../src/agent/fixture.ts";

const { values: opt } = parseArgs({ options: { local: { type: "boolean", default: false }, url: { type: "string" }, n: { type: "string", default: "100" },
  k: { type: "string", default: "8" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const N = Number(opt.n), K = Number(opt.k);
const local = opt.local ? await startLocal() : null;
const U = local ? local.url : baseUrl(opt);
const rows = new Rows("l014", opt.run);
console.log(`L0-14 against ${U}, run ${opt.run}`);
await harness(U, "/m0/setup", {});
await hookReset();
const t0 = Date.now();

const T = { alice: await tok("alice"), bob: await tok("bob"), carol: await tok("carol"), mallory: await tok("mallory"), dave: await tok("dave"),
  ws: await tok("grun-ws"), agent: await tok("grun-agent"), vendor: await tok("vendor"), flagged: await tok("flagged"), vendorUser: await tok("vendor-user"),
  mcp: await tok("mcp-bob"), insights: await tok("insights"), jwtAgent: await tok("jwt-agent"), jwtWs: await tok("jwt-workspace"), jwtClaude: await tok("jwt-claude") };

// Issues: #a assigned to graftyard-agent's bot by alice, #b assigned to another bot (the vendor's) by bob.
const ia = await api(U, "POST", `/api/v3/repos/acme/mono/issues`, T.bob, { title: "assigned to graftyard-agent", body: "fix it" });
const ib = await api(U, "POST", `/api/v3/repos/acme/mono/issues`, T.bob, { title: "assigned to another bot", body: "fix it" });
await api(U, "POST", `/api/v3/repos/acme/mono/issues/${ia.body.number}/assignees`, T.alice, { assignees: ["graftyard-agent[bot]"] });
await api(U, "POST", `/api/v3/repos/acme/mono/issues/${ib.body.number}/assignees`, T.bob, { assignees: ["claude[bot]"] });

// Human writers are paced under DEMOTE_PER_MIN, which counts task creates and cancels like any write (see the results).
const pace = { bob: new Pace(), carol: new Pace() };
const create = (token: string, body: object, headers: Record<string, string> = {}) => api(U, "POST", `${V1}/tasks`, token, body, headers);
type Case = { name: string; token: () => Promise<string>; body: (i: number) => object; headers?: Record<string, string> };
/** Demote dave: 61 writes in under a minute (comments), so his commands take the Agent class. */
async function demote(): Promise<void> {
  await pool(64, 16, (i) => api(U, "POST", `/api/v3/repos/acme/mono/issues/${ia.body.number}/comments`, T.dave, { body: `ping ${i}` }));
}
const creatorCases: Case[] = [
  { name: "installation token (flagged App without confine)", token: async () => T.flagged, body: () => ({ prompt: "p" }) },
  { name: "installation token (vendor App)", token: async () => T.vendor, body: () => ({ prompt: "p" }) },
  { name: "installation token (graftyard-insights)", token: async () => T.insights, body: () => ({ prompt: "p" }) },
  { name: "AgentApp's user token (vendor App, writer carol)", token: async () => T.vendorUser, body: () => ({ prompt: "p" }) },
  { name: "MCP token (bob, a writer)", token: async () => T.mcp, body: () => ({ prompt: "p" }) },
  { name: "demoted user (dave past DEMOTE_PER_MIN)", token: async () => T.dave, body: () => ({ prompt: "p" }) },
  { name: "read-only user (mallory)", token: async () => T.mallory, body: () => ({ prompt: "p" }) },
  { name: "user with X-Graftyard-Agent-Session (bob)", token: async () => T.bob, body: () => ({ prompt: "p" }), headers: { "X-Graftyard-Agent-Session": T1 } },
  { name: "controller from an issue assigned to another bot", token: async () => T.agent, body: () => ({ issue_number: ib.body.number }) },
  { name: "controller with a prompt and no issue", token: async () => T.agent, body: () => ({ prompt: "p" }) },
];

async function runCase(phase: string, c: Case, expectStatus: (s: number) => boolean): Promise<void> {
  if (c.name.startsWith("demoted")) await demote();
  const token = await c.token();
  await pool(N, K, async (i) => {
    if (c.name.startsWith("demoted") && i % 25 === 24) await demote();   // stay past 60 writes a minute
    const r = await create(token, c.body(i), { ...(c.headers ?? {}), "x-m0-probe": `${opt.run}-${phase}-${i}` });
    rows.add(`${phase}/${c.name}`, "refused", r.status, r.status >= 400, { message: msg(r.body), ok: expectStatus(r.status) });
  });
}

// graftyard-workspace has no installation on the base in fork mode, so GitHub's answer is 404 before any task rule.
await pool(N, K, async () => {
  const r = await create(T.ws, { prompt: "p" });
  rows.add("info/installation token (graftyard-workspace, fork mode: not installed on the base)", "refused", r.status, r.status >= 400, { message: msg(r.body) });
});

// ---- Phase 1: agents_enabled off (the default). Every Graftyard-run create is 422, valid creators included. ----
const offCases: Case[] = [...creatorCases,
  { name: "human writer (bob), the valid creator", token: async () => T.bob, body: () => ({ prompt: "p" }) },
  { name: "controller from an issue assigned to its bot", token: async () => T.agent, body: () => ({ issue_number: ia.body.number }) }];
for (const c of offCases) await runCase("off", c, (s) => s === 422);

// ---- Phase 2: on. ----
await orgSet(U, { agents_enabled: true });
for (const c of creatorCases) await runCase("on", c, (s) => s === 403 || s === 422);

// The body that sets `prefix` is accepted (the host ignores it) and the prefix is always derived from the id. A body
// that sets `session` for a Graftyard-run task is refused. Run as the valid creators: bob (prompt) and the controller.
const created: { id: string; t: number; by: string }[] = [];
await pool(N, K, async (i) => {
  await pace.bob.next();
  const t = Date.now();
  const r = await create(T.bob, { prompt: `p ${i}`, prefix: "refs/heads/main/", session: undefined });
  const ok = r.status === 201 && r.body.prefix === `refs/heads/graftyard/${r.body.id}/`;
  rows.add("on/body sets prefix (bob): prefix derived from session", "accepted", r.status, !ok, { message: r.body.prefix ?? msg(r.body) });
  if (r.status === 201) created.push({ id: r.body.id, t, by: "bob" });
});
await pool(N, K, async (i) => {
  const r = await create(T.bob, { prompt: `p ${i}`, session: `mine-${i}` });
  rows.add("on/body sets session for a Graftyard-run task", "refused", r.status, r.status >= 400, { message: msg(r.body) });
});
// Controller creates from the issue assigned to its bot: allowed, with alice (who assigned) as requester. The fixture
// already holds open tasks naming alice (T1, T2, T3, and the vendor's External 3.fix-1), so the control fills alice
// exactly to TASKS_OPEN_PER_REQUESTER and then checks that one more is refused.
const aliceOpen = Object.values(await harness(U, `/m0/tasks?repo=${encodeURIComponent("acme/mono")}`) as Record<string, { requester: number | null; state: string }>)
  .filter((t) => t.requester === 10 && !["Completed", "Failed", "TimedOut", "Cancelled"].includes(t.state)).length;
const nCtl = Math.min(N, 100 - aliceOpen);
console.log(`alice already requests ${aliceOpen} open tasks; the controller control creates ${nCtl}`);
await pool(nCtl, K, async (i) => {
  const t = Date.now();
  const r = await create(T.agent, { issue_number: ia.body.number, prefix: "refs/heads/x/" });
  const ok = r.status === 201 && r.body.prefix === `refs/heads/graftyard/${r.body.id}/` && r.body.requester?.id === 10;
  rows.add("on/controller from an issue assigned to its bot (control)", "accepted", r.status, !ok, { message: r.body.prefix ?? msg(r.body) });
  if (r.status === 201) created.push({ id: r.body.id, t, by: "controller" });
});
{
  const r = await create(T.agent, { issue_number: ia.body.number });
  rows.add("on/controller create past alice's cap", "refused", r.status, r.status >= 400, { message: msg(r.body) });
}
// The row's case: 101 open tasks for one requester. carol creates 100, then the 101st 100 times.
await pool(100, K, async (i) => {
  await pace.carol.next();
  const t = Date.now();
  const r = await create(T.carol, { prompt: `cap ${i}` });
  rows.add("on/carol's first 100 open tasks (control)", "accepted", r.status, r.status !== 201, { message: msg(r.body) });
  if (r.status === 201) created.push({ id: r.body.id, t, by: "carol" });
});
await pace.carol.next();
await pool(N, K, async () => {
  const r = await create(T.carol, { prompt: "the 101st" });
  rows.add("on/101st open task for one requester (carol)", "refused", r.status, r.status >= 400, { message: msg(r.body) });
});

// PATCH of a cancelled task to in_progress (by the task's App), 100 tasks cancelled by their requester.
const toCancel = created.filter((c) => c.by === "carol").slice(0, N);
const cancels: { id: string; t: number }[] = [];
await pool(toCancel.length, K, async (i) => {
  await pace.carol.next();
  const t = Date.now();
  const r = await api(U, "POST", `${V1}/tasks/${toCancel[i].id}/cancel`, T.carol, {});
  rows.add("on/requester cancels (control)", "accepted", r.status, r.status !== 200, { message: r.body.state ?? msg(r.body) });
  if (r.status === 200) cancels.push({ id: toCancel[i].id, t });
});
await pool(toCancel.length, K, async (i) => {
  const r = await api(U, "PATCH", `${V1}/tasks/${toCancel[i].id}`, T.agent, { state: "in_progress" });
  rows.add("on/PATCH of a cancelled task to in_progress", "refused", r.status, r.status >= 400, { message: msg(r.body) });
});
// Also: cancelled set by PATCH (host-only states), and PATCH by another App.
const live = created.filter((c) => c.by === "bob").slice(0, N);
await pool(live.length, K, async (i) => {
  const r = await api(U, "PATCH", `${V1}/tasks/${live[i].id}`, T.agent, { state: "cancelled" });
  rows.add("on/PATCH state cancelled (host-only)", "refused", r.status, r.status >= 400, { message: msg(r.body) });
  const r2 = await api(U, "PATCH", `${V1}/tasks/${live[i].id}`, T.vendor, { state: "in_progress" });
  rows.add("on/PATCH by another App", "refused", r2.status, r2.status >= 400, { message: msg(r2.body) });
});

// Token requests: another App's task, a repository outside the token's list, all repositories, an External task, and
// a cancelled task. Controls: graftyard-workspace on the fork, graftyard-agent on the base.
const liveTask = live[0].id;
const mint = (jwt: string, inst: number, body: object) => api(U, "POST", `/api/v3/app/installations/${inst}/access_tokens`, jwt, body);
const mintCases: [string, string, number, object, "refused" | "accepted"][] = [
  ["mint naming another App's task (vendor JWT)", T.jwtClaude, 103, { repositories: ["mono"], graftyard_task: liveTask }, "refused"],
  ["mint with a repository outside the task's (acme/other)", T.jwtAgent, 101, { repositories: ["mono", "other"], graftyard_task: liveTask }, "refused"],
  ["mint with every repository of the installation", T.jwtAgent, 101, { graftyard_task: liveTask }, "refused"],
  ["mint naming an External task", T.jwtClaude, 103, { repositories: ["mono"], graftyard_task: X1 }, "refused"],
  ["mint naming a cancelled task", T.jwtWs, 102, { repositories: ["acme-agents/mono"], graftyard_task: toCancel[0].id }, "refused"],
  ["mint: graftyard-workspace on the workspace fork (control)", T.jwtWs, 102, { repositories: ["acme-agents/mono"], graftyard_task: liveTask }, "accepted"],
  ["mint: graftyard-agent on the base (control)", T.jwtAgent, 101, { repositories: ["mono"], graftyard_task: liveTask }, "accepted"],
];
for (const [name, jwt, inst, body, expect] of mintCases)
  await pool(N, K, async () => {
    const r = await mint(jwt, inst, body);
    rows.add(`on/${name}`, expect, r.status, r.status >= 400, { message: r.body.graftyard_task ?? msg(r.body) });
  });

// ---- Deliveries: every create and cancel at the controller within 5 s, at no other App's webhook. ----
await new Promise((r) => setTimeout(r, 8_000));
const hooks = await hookLog(t0);
const hostLog = await harness(U, "/m0/deliveries") as { id: string; session: string; action: string; at: number; status?: number; doneAt?: number }[];
const byId = new Map(hooks.map((h) => [h.id, h]));
const expected = [...created.map((c) => ({ id: c.id, action: "created", t: c.t })), ...cancels.map((c) => ({ id: c.id, action: "cancelled", t: c.t }))];
const lat: number[] = [];
let missing = 0, late = 0, badSig = 0;
for (const e of expected) {
  const d = hostLog.find((x) => x.session === e.id && x.action === e.action);
  const h = d ? byId.get(d.id) : undefined;
  if (!h) { missing++; continue; }
  if (h.app !== "graftyard-agent") { missing++; continue; }
  if (!h.sig) badSig++;
  const ms = h.at - d!.at;   // host commit to arrival, both on Cloudflare clocks (the driver's clock is not used)
  lat.push(ms);
  if (ms > 5_000) late++;
}
const others = hooks.filter((h) => h.app !== "graftyard-agent");
const othersTask = others.filter((h) => h.event === "graftyard_task");
lat.sort((a, b) => a - b);
const pct = (p: number) => lat[Math.min(lat.length - 1, Math.floor(p * lat.length))];
const delivery = { expected: expected.length, atController: lat.length, missing, late, badSig, p50: pct(0.5), p99: pct(0.99), max: lat.at(-1),
  graftyardTaskAtOtherApps: othersTask.length, anyAtOtherApps: others.length, hostDeliveryTargets: [...new Set(hostLog.map((d: any) => d.url))] };
console.log(`deliveries: ${JSON.stringify(delivery)}`);

rows.print();
const bad = rows.bad();
console.log(`\ncells off their expectation: ${bad.length}`);
for (const b of bad) console.log(`  ${b}`);
writeFileSync(`${RAW}/wp13-l014-${opt.run}-summary.json`, JSON.stringify({ run: opt.run, url: local ? "local" : U, n: N, cells: rows.summary(), delivery, bad }, null, 1));
local?.stop();
