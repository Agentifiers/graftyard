// AM2-WP4 hosted: L0-15's `AgentGate` half (`build/BUILD-PLAN.md`, AM2-WP4, LM-2) on a fresh repository of graft-m3f,
// through graft-mcp. m3-ci is flagged a confined ThirdParty App (`refs/heads/claude/**`) and opens agent PRs under two
// external tasks: one whose requester is m3-admin, one whose requester is m3-member. With no `extra_human_approvals`
// each PR needs one approval from a human writer other than its requester, so a single counted approval would merge it.
//   mcp        m3-member approves each m3-admin-requested PR through graft-mcp's `approve_pull_request` on a
//              `write:write_approve_merge` grant; the review must be accepted, and `merge_pull_request` and an admin
//              REST merge must both be refused by `graftyard/agent-gate`
//   requester  m3-member, the requester, approves each of its own task's PRs with its PAT; the admin's REST merge is refused
//   controls   the first PR of each case then gets an approval that counts (m3-member's PAT on the mcp PR, m3-admin's on
//              the requester PR) and merges, so a refusal above is the gate and not something else
// Environment as `tests/am2/hosted/mcp.mjs`, plus AM2_REPO (the day's fresh repository) and L015_N (PRs per case, 20).
//   node tests/am2/hosted/l015.mjs <tag> run | unflag
// Rows go to tests/am2/raw/l015-<tag>.jsonl and a summary to stdout. No token reaches a log line (`scan`).
import { mkdirSync, appendFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { api, ghs, Git, log, scan, state, tokens } from "../../am1/hosted/lib.mjs";
import { REPO, login, mcpCall } from "./mcp.mjs";

const [tag, step] = process.argv.slice(2);
if (!tag || !["run", "unflag"].includes(step)) throw new Error("usage: l015.mjs <tag> run|unflag");
const N = Number(process.env.L015_N ?? 20);
mkdirSync("tests/am2/raw", { recursive: true });
const OUT = `tests/am2/raw/l015-${tag}.jsonl`;
const row = (v) => appendFileSync(OUT, scan(JSON.stringify({ at: new Date().toISOString(), step, ...v })) + "\n");
const [owner, repo] = REPO.split("/");
const V1 = `/api/graftyard/v1/repos/${REPO}`;
const gql = (tok, query, variables, extra) => api("POST", "/api/graphql", tok, { query, variables }, extra);
const GATE = /agent-gate/;

/** One tool call through graft-mcp on the 2026-07-28 lane: `{status, isError, value}`. */
async function tool(bearer, name, args) {
  const r = await mcpCall(bearer, "tools/call", { name, arguments: { owner, repo, ...args } });
  let body = null;
  try { body = JSON.parse(r.text); } catch { /* left null */ }
  const res = body?.result;
  let value = null;
  try { value = JSON.parse(res?.content?.[0]?.text ?? "null"); } catch { value = res?.content?.[0]?.text ?? null; }
  return { status: r.status, isError: !!res?.isError, value, error: body?.error };
}

async function approvePat(tok, n) {
  const q = await gql(tok, `query { repository(owner: "${owner}", name: "${repo}") { pullRequest(number: ${n}) { id } } }`);
  const pid = q.body?.data?.repository?.pullRequest?.id;
  const a = await gql(tok, "mutation($i: AddPullRequestReviewInput!) { addPullRequestReview(input: $i) { pullRequestReview { id state } } }",
    { i: { pullRequestId: pid, event: "APPROVE" } });
  return { status: a.status, state: a.body?.data?.addPullRequestReview?.pullRequestReview?.state, errors: a.body?.errors };
}
const restMerge = (n, sha) => api("PUT", `/api/v3/repos/${REPO}/pulls/${n}/merge`, tokens.admin, { merge_method: "merge", sha });

if (step === "unflag") {
  const un = await api("DELETE", `/api/graftyard/v1/agents/apps/${state.app}`, tokens.admin);
  row({ case: "unflag", status: un.status });
  log(`unflag: ${un.status}`);
}

if (step === "run") {
  const fl = await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, tokens.admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  row({ case: "flag", status: fl.status });
  if (fl.status !== 200 && fl.status !== 201) throw new Error(`flag: ${fl.status}`);
  const inst = (await ghs({})).body.token;
  const tasks = {};
  for (const [c, who] of [["mcp", "m3-admin"], ["requester", "m3-member"]]) {
    const t = await api("POST", `${V1}/tasks`, inst, { kind: "external", session: `l015-${tag}-${c}`, summary: "am2 l015", requester: who });
    // The reply may render the requester's login as `ghost` (day l015: a directory miss), so the stored requester is
    // checked by account id in the unfiltered list (the `requester=` filter answered 503 intermittently).
    const want = c === "mcp" ? state.admin : state.member;
    const l = await api("GET", `${V1}/tasks`, tokens.admin);
    const mine = (Array.isArray(l.body) ? l.body : l.body?.tasks ?? []).find((x) => x.id === t.body?.id);
    row({ case: `task ${c}`, status: t.status, id: t.body?.id, replyRequester: t.body?.requester?.login, requesterId: mine?.requester?.id, want });
    if (t.status !== 201 || mine?.requester?.id !== want) throw new Error(`task ${c}: ${t.status} requester ${mine?.requester?.id} want ${want}`);
    tasks[c] = t.body.id;
  }
  const g = new Git(REPO).as(inst);
  if (!g.clone().ok) throw new Error("clone failed");
  const main = g.run(["rev-parse", "HEAD"]).out.trim();
  const rid = (await gql(tokens.admin, `query { repository(owner: "${owner}", name: "${repo}") { id } }`)).body?.data?.repository?.id;
  /** m3-ci's PR on `claude/l015-<c>-<k>` in task `c`'s session: `{n, sha}`. */
  async function agentPr(c, k) {
    const branch = `claude/l015-${tag}-${c}-${k}`;
    g.run(["checkout", "-q", "-B", branch, main]);
    const f = join(g.wt, "l015", `${c}-${k}.txt`);
    mkdirSync(join(f, ".."), { recursive: true });
    writeFileSync(f, `${c} ${k} ${Date.now()}\n`);
    g.run(["add", "-A"]);
    g.run(["commit", "-q", "-m", `l015 ${c} ${k}`]);
    const sha = g.run(["rev-parse", "HEAD"]).out.trim();
    const p = g.run(["push", "-q", "origin", `HEAD:refs/heads/${branch}`]);
    if (!p.ok) throw new Error(`push ${branch}: ${p.out.slice(0, 200)}`);
    const pr = await gql(inst, "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
      { i: { repositoryId: rid, baseRefName: "main", headRefName: branch, title: `l015 ${c} ${k}`, body: "l015", draft: false } },
      { "x-graftyard-agent-session": tasks[c] });
    const n = pr.body?.data?.createPullRequest?.pullRequest?.number;
    if (!n) throw new Error(`pr ${branch}: ${pr.status} ${JSON.stringify(pr.body?.errors).slice(0, 200)}`);
    return { n, sha };
  }

  const t = await login("write:write_approve_merge tools:pull_requests");
  row({ case: "login", scope: t.scope });
  const tk = t.access_token;
  const count = { mcp: { approved: 0, mcpMergeRefused: 0, restMergeRefused: 0 }, requester: { approved: 0, restMergeRefused: 0 } };
  const control = {};
  for (let k = 0; k < N; k++) {
    // The MCP case: m3-member's approval through Graftyard MCP on a PR m3-admin requested.
    const a = await agentPr("mcp", k);
    const ap = await tool(tk, "approve_pull_request", { number: a.n, body: "l015" });
    const approved = !ap.isError && ap.value?.state === "APPROVED";
    const mm = await tool(tk, "merge_pull_request", { number: a.n, sha: a.sha, method: "merge" });
    const mmRefused = mm.isError && GATE.test(JSON.stringify(mm.value));
    const rm = await restMerge(a.n, a.sha);
    const rmRefused = rm.status !== 200 && GATE.test(rm.body?.message ?? "");
    count.mcp.approved += approved; count.mcp.mcpMergeRefused += mmRefused; count.mcp.restMergeRefused += rmRefused;
    row({ case: "mcp", k, n: a.n, approve: { isError: ap.isError, state: ap.value?.state }, mcpMerge: { isError: mm.isError, value: mm.value },
      restMerge: { status: rm.status, msg: rm.body?.message } });
    if (k === 0) {
      const own = await approvePat(tokens.member, a.n);
      const m = await restMerge(a.n, a.sha);
      control.mcp = { approve: own, merge: m.status, merged: m.body?.merged };
      row({ case: "control mcp", n: a.n, ...control.mcp, msg: m.body?.message });
    }
    // The requester case: m3-member approves its own task's PR with its PAT.
    const b = await agentPr("requester", k);
    const own = await approvePat(tokens.member, b.n);
    const rb = await restMerge(b.n, b.sha);
    const rbRefused = rb.status !== 200 && GATE.test(rb.body?.message ?? "");
    count.requester.approved += own.state === "APPROVED"; count.requester.restMergeRefused += rbRefused;
    row({ case: "requester", k, n: b.n, approve: own, restMerge: { status: rb.status, msg: rb.body?.message } });
    if (k === 0) {
      const adm = await approvePat(tokens.admin, b.n);
      const m = await restMerge(b.n, b.sha);
      control.requester = { approve: adm, merge: m.status, merged: m.body?.merged };
      row({ case: "control requester", n: b.n, ...control.requester, msg: m.body?.message });
    }
    log(`k ${k}: mcp approved ${approved} refused ${mmRefused}/${rmRefused}; requester approved ${own.state} refused ${rbRefused}`);
  }
  const pass = {
    mcp: count.mcp.approved === N && count.mcp.mcpMergeRefused === N && count.mcp.restMergeRefused === N,
    requester: count.requester.approved === N && count.requester.restMergeRefused === N,
    controls: control.mcp?.merge === 200 && control.requester?.merge === 200,
  };
  row({ case: "summary", N, count, control, pass });
  log(`summary: ${JSON.stringify({ N, count, control: { mcp: control.mcp?.merge, requester: control.requester?.merge }, pass })}`);
}
