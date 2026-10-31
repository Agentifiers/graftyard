// M0-WP15, L0-11: the unmodified-client workload through the recorder (run/wp15-tee.ts at https://ghes.wp15.test:8443).
// Three clients, none of them changed or configured beyond the GHES host and a token:
//   gh 2.102.1 with GH_HOST            issue/pr view and list, repo view, api (REST and GraphQL), issue comment
//   octokit (@octokit/rest, graphql)   the same reads, comments, and GraphQL through octokit's baseUrl
//   github-mcp-server v2.0.3, local    default toolsets over stdio, GITHUB_HOST set, driven by an MCP stdio client
// It runs rounds until the recorder holds `--target` requests (default 1,000). Writes are serialized by the recorder,
// so both deployments see the same sequence.
//   node wp15/l011-workload.ts [--target 1000]

import { execFile } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import { promisify } from "node:util";
import { Octokit } from "@octokit/rest";
import { Client } from "sdkv1/client/index.js";
import { StdioClientTransport } from "sdkv1/client/stdio.js";
import { tok, ROOT } from "../run/wp15.ts";

const run = promisify(execFile);
const target = Number(process.argv[process.argv.indexOf("--target") + 1] || 1000);
const LOG = `${ROOT}results/raw/wp15-l011.jsonl`;
const lines = () => existsSync(LOG) ? readFileSync(LOG, "utf8").split("\n").filter(Boolean).length : 0;
const HOST = "ghes.wp15.test:8443";
const T = await tok("alice");
const env = { ...process.env, NO_PROXY: `ghes.wp15.test,raw.ghes.wp15.test,${process.env.NO_PROXY ?? ""}`, no_proxy: `ghes.wp15.test,raw.ghes.wp15.test,${process.env.no_proxy ?? ""}`,
  SSL_CERT_FILE: `${homedir()}/.cache/wp15/tls/bundle.pem`, NODE_EXTRA_CA_CERTS: `${homedir()}/.cache/wp15/tls/cert.pem` };
process.env.NODE_EXTRA_CA_CERTS = env.NODE_EXTRA_CA_CERTS;
const errs: Record<string, number> = {};
const note = (k: string, e: unknown) => { errs[k] = (errs[k] ?? 0) + 1; if (errs[k] === 1) console.error(k, String((e as Error).message ?? e).slice(0, 300)); };

// ---- gh ----
const ghEnv = { ...env, GH_CONFIG_DIR: "/tmp/wp15-ghcfg", GH_HOST: HOST, GH_ENTERPRISE_TOKEN: T, GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" };
const R = `${HOST}/acme/mono`;
const ghCmds = (i: number): string[][] => [["api", "/user"], ["issue", "view", "1", "-R", R], ["issue", "list", "-R", R], ["pr", "list", "-R", R], ["pr", "view", "2", "-R", R],
  ["repo", "view", R, "--json", "name,defaultBranchRef"], ["api", "repos/acme/mono/git/ref/heads/main"], ["api", "graphql", "-f", "query=query{viewer{login} rateLimit{remaining}}"],
  ["issue", "comment", "1", "-R", R, "--body", `gh comment ${i}`], ["api", "/rate_limit"]];
async function gh(i: number) { for (const c of ghCmds(i)) { try { await run("gh", c, { env: ghEnv, timeout: 30_000 }); } catch (e) { note(`gh ${c.slice(0, 2).join(" ")}`, e); } } }

// ---- octokit ----
const ok = new Octokit({ auth: T, baseUrl: `https://${HOST}/api/v3`, userAgent: "wp15-octokit" });
async function octo(i: number) {
  const o = { owner: "acme", repo: "mono" };
  const calls: [string, () => Promise<unknown>][] = [
    ["users.getAuthenticated", () => ok.rest.users.getAuthenticated()], ["repos.get", () => ok.rest.repos.get(o)], ["issues.get", () => ok.rest.issues.get({ ...o, issue_number: 1 })],
    ["issues.listComments", () => ok.rest.issues.listComments({ ...o, issue_number: 1 })], ["pulls.list", () => ok.rest.pulls.list(o)], ["pulls.get", () => ok.rest.pulls.get({ ...o, pull_number: 2 })],
    ["git.getRef", () => ok.rest.git.getRef({ ...o, ref: "heads/feature" })], ["issues.listForRepo", () => ok.rest.issues.listForRepo(o)], ["rateLimit.get", () => ok.rest.rateLimit.get()],
    ["issues.createComment", () => ok.rest.issues.createComment({ ...o, issue_number: 1, body: `octokit comment ${i}` })],
    ["pulls.createReview", () => ok.rest.pulls.createReview({ ...o, pull_number: 2, event: "COMMENT", body: `review ${i}` })],
    ["checks.create", () => ok.rest.checks.create({ ...o, name: "ci", head_sha: "788fb484c3b3143ce0c91f8631c01cedb2b22c9b", status: "completed", conclusion: "success" })],
    ["graphql", () => ok.graphql(`query($o:String!,$n:String!){repository(owner:$o,name:$n){nameWithOwner issue(number:1){title comments(first:5){totalCount}} pullRequest(number:2){headRefName isDraft}}}`, { o: "acme", n: "mono" })],
    ["issues.get 404", () => ok.rest.issues.get({ ...o, issue_number: 999 })], ["repos.get 404", () => ok.rest.repos.get({ owner: "acme", repo: "nope" })] ];
  for (const [k, f] of calls) { try { await f(); } catch (e) { if (!k.endsWith("404")) note(`octokit ${k}`, e); } }
}

// ---- github-mcp-server (default toolsets) ----
async function ghmcp(rounds: number) {
  const t = new StdioClientTransport({ command: `${homedir()}/.cache/wp15/github-mcp-server`, args: ["stdio"], env: { ...env, GITHUB_PERSONAL_ACCESS_TOKEN: T, GITHUB_HOST: `https://${HOST}` } as Record<string, string>, stderr: "ignore" });
  const c = new Client({ name: "wp15-l011", version: "1" });
  await c.connect(t);
  const tools = (await c.listTools()).tools.map((x) => x.name);
  const o = { owner: "acme", repo: "mono" };
  const calls: [string, Record<string, unknown>][] = [["get_me", {}], ["issue_read", { ...o, method: "get", issue_number: 1 }], ["issue_read", { ...o, method: "get_comments", issue_number: 1 }],
    ["list_issues", o], ["list_pull_requests", o], ["pull_request_read", { ...o, method: "get", pullNumber: 2 }], ["list_branches", o], ["search_issues", { query: "repo:acme/mono hello" }],
    ["add_issue_comment", { ...o, issue_number: 1, body: "mcp comment" }]];
  const used: Record<string, number> = {};
  for (let i = 0; i < rounds && lines() < target; i++) for (const [n, a] of calls) {
    if (!tools.includes(n)) { used[`${n} (absent)`] = 1; continue; }
    try { const r = await c.callTool({ name: n, arguments: a }); used[n] = (used[n] ?? 0) + 1; if (r.isError) note(`mcp ${n} isError`, JSON.stringify(r.content).slice(0, 200)); } catch (e) { note(`mcp ${n}`, e); }
  }
  await c.close();
  return { tools, used };
}

let round = 0;
const mcpInfo = await ghmcp(3);
console.log("github-mcp-server tools:", mcpInfo.tools.length, JSON.stringify(mcpInfo.used));
while (lines() < target) {
  round++;
  await gh(round);
  if (lines() >= target) break;
  await octo(round);
  if (lines() >= target) break;
  if (round % 3 === 0) await ghmcp(1);
  console.log(`round ${round}: ${lines()} recorded`);
}
console.log(JSON.stringify({ rounds: round, recorded: lines(), errors: errs }));
