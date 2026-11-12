// B0-36's label-mutation part over GraphQL (`build/BUILD-PLAN.md`, B0-36, "a label outside the allowlist ... over git,
// REST, and GraphQL"), which `am1-runs` could not run because the deployed edge did not route `addLabelsToLabelable`
// (fixed in `89cf205`). Per agent kind, N `addLabelsToLabelable` calls under a minted installation token, each naming a
// label outside the repository's agent label allowlist, must be refused by the agent rule; a label inside the
// allowlist is the control and must be accepted, so a refusal that came from anything but the allowlist would show.
// Kinds: the confined vendor App (m3-ci, ThirdParty with `confine refs/heads/claude/**`), the Graftyard-run App
// (`graftyard-agent`) with its shipped permissions and widened to every permission, and `Graftyard MCP`, which is
// never an `Agent` and so is never refused by the rule.
//
//   node tests/am1/lf4/b036-labels.mjs --repo NAME [--n 100]
// Environment as tests/am1/hosted/lib.mjs. Rows: tests/am1/raw/b036-labels.jsonl.
import { parseArgs } from "node:util";
import { OP, api, ghs, log, must, row, sleep, state, tokens } from "../hosted/lib.mjs";
import { agentToken } from "../rest/apps.mjs";
import { execFileSync } from "node:child_process";

const { values: opt } = parseArgs({ options: { repo: { type: "string" }, n: { type: "string" } } });
if (!opt.repo) throw new Error("usage: b036-labels.mjs --repo NAME [--n 100]");
const N = Number(opt.n ?? 100);
const R = `m3org/${opt.repo}`;
const OUT = new URL("../raw/b036-labels.jsonl", import.meta.url).pathname;
const tag = Date.now().toString(36);
const admin = tokens.admin;
const gql = (tok, query, variables) => api("POST", "/api/graphql", tok, { query, variables });
const count = (xs) => xs.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {});

// The policy's allowlist holds `lf4-ok-<tag>`; `lf4-no-<tag>` is outside it.
const [ok, no] = [`lf4-ok-${tag}`, `lf4-no-${tag}`];
const ids = {};
for (const name of [ok, no]) {
  const r = await api("POST", `/api/v3/repos/${R}/labels`, admin, { name, color: "ededed" });
  if (r.status !== 201) throw new Error(`label ${name}: ${r.status}`);
  ids[name] = r.body.node_id;
}
const pol = await api("PUT", `/api/graftyard/v1/repos/${R}/agents/policy`, admin, { labels: [ok] });
row(OUT, { step: "policy", status: pol.status, labels: pol.body?.labels });
// m3-ci's `AgentApp` row, as the operator on the install's chain the way tests/am1/rest/apps.mjs writes its rows: on
// graft-m3c the m3 admin is not a site admin of `authz:1`, so `PUT /api/graftyard/v1/agents/apps/{id}` answers 403.
const flag = await must("authz:1", { cred: OP, op: { PutAgentApp: { app: state.app, kind: "ThirdParty", confine: "refs/heads/claude/**" } } });
row(OUT, { step: "flag m3-ci confined", ok: !!flag?.ok });
await sleep(2000);
const issue = (await gql(admin, "mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { id number } } }",
  { i: { repositoryId: (await gql(admin, `query { repository(owner: "m3org", name: "${opt.repo}") { id } }`)).body.data.repository.id, title: `b036 labels ${tag}`, body: "b036" } })).body.data.createIssue.issue;

const ADD = "mutation($i: AddLabelsToLabelableInput!) { addLabelsToLabelable(input: $i) { labelable { __typename } } }";
const REMOVE = "mutation($i: RemoveLabelsFromLabelableInput!) { removeLabelsFromLabelable(input: $i) { labelable { __typename } } }";
const res = (r, f) => (r.body?.data?.[f] ? "accepted" : `${r.status} ${String(r.body?.errors?.[0]?.message ?? "").slice(0, 90)}`);
const kinds = [
  ["m3-ci (confined vendor App)", () => ghs({})],
  ["graftyard-agent (shipped)", () => agentToken("graftyard-agent")],
  ["graftyard-agent (widened)", () => agentToken("graftyard-agent"), "wide"],
  ["Graftyard MCP (control)", () => agentToken("graftyard-mcp")],
];
const out = {};
for (const [kind, mint, widen] of kinds) {
  if (widen) { execFileSync("node", ["tests/am1/runs/widen.mjs", "wide"], { stdio: "ignore" }); await sleep(3000); }
  const m = await mint();
  row(OUT, { step: "token", kind, status: m.status, permissions: m.body?.permissions });
  if (m.status !== 201) { out[kind] = { mint: m.status }; continue; }
  const tok = m.body.token;
  const outside = [], inside = [];
  for (let i = 0; i < N; i++) {
    outside.push(res(await gql(tok, ADD, { i: { labelableId: issue.id, labelIds: [ids[no]] } }), "addLabelsToLabelable"));
    if (outside.at(-1) === "accepted") await gql(admin, REMOVE, { i: { labelableId: issue.id, labelIds: [ids[no]] } });
  }
  for (let i = 0; i < 5; i++) {
    inside.push(res(await gql(tok, ADD, { i: { labelableId: issue.id, labelIds: [ids[ok]] } }), "addLabelsToLabelable"));
    await gql(admin, REMOVE, { i: { labelableId: issue.id, labelIds: [ids[ok]] } });
  }
  out[kind] = { outside: count(outside), inside: count(inside) };
  row(OUT, { kind, ...out[kind] });
  log(`${kind}: ${JSON.stringify(out[kind])}`);
  if (widen) execFileSync("node", ["tests/am1/runs/widen.mjs", "shipped"], { stdio: "ignore" });
}
// The label the refused calls named never reached the issue.
const after = await api("GET", `/api/v3/repos/${R}/issues?state=open&per_page=100`, admin);
const labels = (after.body ?? []).find((x) => x.number === issue.number)?.labels?.map((l) => l.name) ?? null;
row(OUT, { step: "final labels", labels });
log(`b036 labels ${JSON.stringify({ out, labels })}`);
