// M0-WP15, L0-07: drive the prototype host the way each vendor's documentation says its GHES App behaves, with stock
// clients only (octokit 22 with @octokit/auth-app 8, git over HTTPS, github-mcp-server v2.0.3): no Graftyard-specific
// client code. The vendor's cloud is replaced by this script; the host is the prototype, not GHES.
//   node wp15/l007-vendor.ts claude|cursor|codex|ghmcp [--pushes 100]   -> results/raw/wp15-l007-<vendor>.json
// Sources (fetched 2026-10-30): code.claude.com/docs/en/github-enterprise-server.md (manifest, permissions, events),
// cursor.com/docs/integrations/github.md (GHES registration, permissions, REST and GraphQL), help.openai.com 20001248
// (ChatGPT GitHub Enterprise app template: manual App, callback, private key, webhook URL and secret, events).

import { execFile } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { promisify } from "node:util";
import { Octokit } from "@octokit/rest";
import { createAppAuth } from "@octokit/auth-app";
import { Client } from "sdkv1/client/index.js";
import { StdioClientTransport } from "sdkv1/client/stdio.js";
import { ROOT, TOKEN, URLS, tok } from "../run/wp15.ts";

const run = promisify(execFile);
const vendor = process.argv[2] as "claude" | "cursor" | "codex" | "ghmcp";
const PUSHES = Number(process.argv[process.argv.indexOf("--pushes") + 1] || 100);
const H = process.argv.includes("--base") ? URLS.base : URLS.host;
const API = `${H}/api/v3`;
const REPO = { owner: "acme", repo: "mono" };
const HOOKS = `${URLS.aux}/hooks`;
const log: Record<string, unknown> = { vendor, host: H, at: new Date().toISOString(), steps: [] as unknown[] };
const step = (name: string, ok: boolean, detail: unknown = null) => { (log.steps as unknown[]).push({ name, ok, detail }); console.log(ok ? "ok  " : "FAIL", name, detail ? JSON.stringify(detail).slice(0, 300) : ""); };

// ---- each vendor's App, as its documentation says ----
const MANIFESTS: Record<string, Record<string, unknown>> = {
  // Claude: "generates a GitHub App manifest and redirects you to your GHES instance". Permissions and events verbatim.
  claude: { name: "Claude", url: "https://claude.ai", hook_attributes: { url: `${HOOKS}/claude` }, redirect_url: `${URLS.aux}/vendor/claude/callback`,
    public: false, default_permissions: { contents: "write", pull_requests: "write", issues: "write", checks: "write", actions: "read", statuses: "read",
      repository_hooks: "write", metadata: "read", members: "read" },
    default_events: ["pull_request", "issue_comment", "pull_request_review_comment", "pull_request_review", "check_run", "status"] },
  // Cursor: "Register" creates "the Cursor Enterprise Application" on GHES; the permission table, mapped to GitHub names.
  cursor: { name: "Cursor Enterprise", url: "https://cursor.com", hook_attributes: { url: `${HOOKS}/cursor-enterprise` }, redirect_url: `${URLS.aux}/vendor/cursor/callback`,
    public: false, default_permissions: { contents: "write", pull_requests: "write", issues: "write", checks: "write", statuses: "write", actions: "write",
      workflows: "write", administration: "read", organization_custom_roles: "read", organization_custom_properties: "read", metadata: "read" },
    default_events: ["pull_request", "pull_request_review", "issue_comment", "check_run", "check_suite", "status", "push"] },
  // Codex (ChatGPT template): a manually created App. The doc's write-workflow permissions: Contents, Pull requests,
  // Issues, Actions write, Metadata read; no Checks permission is listed. Events: the doc's "common events".
  codex: { name: "ChatGPT GitHub Enterprise", url: "https://chatgpt.com", hook_attributes: { url: `${HOOKS}/chatgpt-github-enterprise` },
    callback_urls: ["https://chatgpt.com/connector/oauth/m0wp15"], request_oauth_on_install: true,
    default_permissions: { contents: "write", pull_requests: "write", issues: "write", actions: "write", metadata: "read" },
    default_events: ["installation", "installation_repositories", "push", "pull_request", "issues", "issue_comment", "workflow_run", "workflow_job"] },
  // github-mcp-server has no App of its own; the operator gives it a token. Here: a plain App with the default
  // toolsets' needs, and its installation token as GITHUB_PERSONAL_ACCESS_TOKEN.
  ghmcp: { name: "MCP Bot", url: "https://github.com/github/github-mcp-server", hook_attributes: { url: `${HOOKS}/mcp-bot` },
    default_permissions: { contents: "write", pull_requests: "write", issues: "write", checks: "write", metadata: "read" }, default_events: ["pull_request", "check_run"] },
};
// --with-checks: the control for Codex, whose template documents no Checks permission (an admin setting, not a patch).
if (process.argv.includes("--with-checks")) (MANIFESTS[vendor].default_permissions as Record<string, string>).checks = "write";
const PREFIX: Record<string, string> = { claude: "claude/", cursor: "cursor/", codex: "codex/", ghmcp: "mcp-bot/" };

async function m0(path: string, body?: unknown, method = "POST"): Promise<any> {
  const r = await fetch(`${H}${path}`, { method, headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const t = await r.text(); if (!r.ok) throw new Error(`${path}: ${r.status} ${t.slice(0, 300)}`);
  try { return JSON.parse(t); } catch { return t; }
}

// 1. Create the App.
type Created = { id: number; slug: string; client_id: string; client_secret: string; webhook_secret: string; pem: string };
async function create(): Promise<Created> {
  const m = MANIFESTS[vendor];
  if (vendor === "claude" || vendor === "cursor") {
    // The manifest flow: the vendor page posts `manifest` to /settings/apps/new; the org owner clicks Create; GHES
    // redirects to redirect_url?code=; the vendor converts the code. The admin's browser session is a cookie.
    const admin = encodeURIComponent(await tok("alice"));
    const state = Math.random().toString(36).slice(2);
    const form = await fetch(`${H}/organizations/acme/settings/apps/new?state=${state}`);
    step("manifest form served, framing denied", form.status === 200 && form.headers.get("x-frame-options") === "deny", { status: form.status });
    const r = await fetch(`${H}/organizations/acme/settings/apps/new`, { method: "POST", redirect: "manual", headers: { cookie: `user_session=${admin}`, "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ manifest: JSON.stringify(m), state }).toString() });
    const loc = new URL(r.headers.get("location") ?? "http://x/");
    step("Create GitHub App redirects to redirect_url with code and state", r.status === 302 && loc.searchParams.get("state") === state && !!loc.searchParams.get("code"), { status: r.status, location: `${loc.origin}${loc.pathname}` });
    const code = loc.searchParams.get("code")!;
    const conv = await new Octokit({ baseUrl: API }).request("POST /app-manifests/{code}/conversions", { code });
    const again = await new Octokit({ baseUrl: API }).request("POST /app-manifests/{code}/conversions", { code }).then(() => 201, (e) => e.status);
    step("conversion returns id, client secret, webhook secret, pem; second use refused", !!conv.data.pem && again === 404, { id: conv.data.id, slug: conv.data.slug, again });
    return conv.data as unknown as Created;
  }
  // Manual "Register new GitHub App" (the Codex template's path, and how an operator would create github-mcp-server's).
  const c = await m0("/_m0/apps", m);
  step("App registered through the manual form", !!c.pem, { id: c.id, slug: c.slug });
  return c;
}

// 2. Install on selected repositories; site admin marks it an AgentApp with its prefix (design A2 step 2).
async function install(app: Created): Promise<number> {
  const agent = { kind: "ThirdParty", confine: `refs/heads/${PREFIX[vendor]}**` };
  const i = await m0(`/_m0/apps/${app.slug}/install`, { repositories: ["mono"], agent_app: agent });
  step("installed on acme/mono", !!i.id, { installation: i.id, agent_app: agent });
  return i.id;
}

async function hooks(slug: string): Promise<{ event: string; ok: number }[]> {
  const r = await fetch(`${HOOKS}/${slug}`, { headers: { authorization: `Bearer ${TOKEN}` } });
  return r.ok ? r.json() as Promise<{ event: string; ok: number }[]> : [];
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function git(args: string[], cwd: string, token: string): Promise<{ ok: boolean; out: string }> {
  const env = { ...process.env, GIT_TERMINAL_PROMPT: "0", GIT_AUTHOR_NAME: `${vendor}[bot]`, GIT_AUTHOR_EMAIL: `${vendor}@example.com`, GIT_COMMITTER_NAME: `${vendor}[bot]`, GIT_COMMITTER_EMAIL: `${vendor}@example.com` };
  const url = `https://x-access-token:${token}@${H.replace("https://", "")}/acme/mono.git`;
  try { const r = await run("git", args.map((a) => a === "URL" ? url : a), { cwd, env, timeout: 60_000 }); return { ok: true, out: r.stdout + r.stderr }; }
  catch (e: any) { return { ok: false, out: String(e.stderr ?? e.message).slice(0, 400) }; }
}

async function main() {
  await fetch(`${HOOKS}/${vendor}`, { method: "DELETE", headers: { authorization: `Bearer ${TOKEN}` } });
  const app = await create();
  await fetch(`${HOOKS}/${app.slug}`, { method: "DELETE", headers: { authorization: `Bearer ${TOKEN}` } });
  const inst = await install(app);
  // The vendor's backend: App JWT (RS256) -> installation token, as @octokit/auth-app does it.
  const appKit = new Octokit({ baseUrl: API, authStrategy: createAppAuth, auth: { appId: app.id, privateKey: app.pem } });
  const me = await appKit.request("GET /app");
  const insts = await appKit.request("GET /app/installations");
  const repoInst = await appKit.request("GET /repos/{owner}/{repo}/installation", REPO);
  step("App JWT: GET /app, /app/installations, /repos/{o}/{r}/installation", me.data.slug === app.slug && insts.data.some((i: any) => i.id === inst) && repoInst.data.id === inst, { installations: insts.data.length });
  const auth = createAppAuth({ appId: app.id, privateKey: app.pem, request: appKit.request });
  const it = await auth({ type: "installation", installationId: inst, repositoryNames: ["mono"] }) as { token: string; expiresAt: string; permissions: Record<string, string> };
  step("installation token minted (POST /app/installations/{id}/access_tokens)", !!it.token, { expiresAt: it.expiresAt, permissions: it.permissions });
  const kit = new Octokit({ baseUrl: API, auth: it.token });

  // 3. Push a branch under its prefix, open a PR.
  const dir = mkdtempSync(`${tmpdir()}/wp15-l007-`);
  const branch = `${PREFIX[vendor]}fix-${Date.now().toString(36)}`;
  let r = await git(["clone", "-q", "URL", "."], dir, it.token);
  step("git clone with the installation token", r.ok, r.ok ? null : r.out);
  await run("git", ["checkout", "-qb", branch], { cwd: dir });
  writeFileSync(`${dir}/docs-shared.md`, `shared by ${vendor}\n`);
  writeFileSync(`${dir}/${vendor}.txt`, `${vendor} change\n`);
  await run("git", ["add", "."], { cwd: dir });
  await run("git", ["-c", `user.name=${vendor}[bot]`, "-c", `user.email=${vendor}@example.com`, "commit", "-qm", `${vendor}: change`], { cwd: dir });
  r = await git(["push", "-q", "URL", `HEAD:refs/heads/${branch}`], dir, it.token);
  step(`git push to ${branch}`, r.ok, r.ok ? null : r.out);
  const outside = await git(["push", "-q", "URL", `HEAD:refs/heads/elsewhere-${vendor}-${Date.now().toString(36)}`], dir, it.token);
  step("push outside the confine prefix refused (AgentApp with confine)", !outside.ok, outside.ok ? "accepted" : outside.out.split("\n").find((l) => l.includes("refused")) ?? outside.out.slice(0, 160));
  const head = (await run("git", ["rev-parse", "HEAD"], { cwd: dir })).stdout.trim();
  let pr: number | null = null;
  try { const p = await kit.rest.pulls.create({ ...REPO, head: branch, base: "main", title: `${vendor}: change`, body: "Opened by the L0-07 driver." }); pr = p.data.number; step("PR opened (REST)", true, { number: pr }); }
  catch (e: any) { step("PR opened (REST)", false, e.status + " " + e.message); }

  // 4. Read graftyard/overlap on the head SHA through the plain Checks API; write its own check run.
  await sleep(1500);
  let overlap: any = null;
  try { const c = await kit.rest.checks.listForRef({ ...REPO, ref: head }); overlap = c.data.check_runs.find((x: any) => x.name === "graftyard/overlap") ?? null;
        step("graftyard/overlap read through GET /commits/{sha}/check-runs", !!overlap, overlap ? { conclusion: overlap.conclusion, title: overlap.output.title, summary: overlap.output.summary } : { names: c.data.check_runs.map((x: any) => x.name) }); }
  catch (e: any) { step("graftyard/overlap read through GET /commits/{sha}/check-runs", false, `${e.status} ${e.message}`); }
  try { const w = await kit.rest.checks.create({ ...REPO, name: `${vendor} review`, head_sha: head, status: "completed", conclusion: "success", output: { title: "ok", summary: "no findings" } });
        step("check run written (POST /check-runs)", w.status === 201, { id: w.data.id }); }
  catch (e: any) { step("check run written (POST /check-runs)", false, `${e.status} ${e.message}`); }
  const gql = await fetch(`${H}/api/graphql`, { method: "POST", headers: { authorization: `bearer ${it.token}` }, body: JSON.stringify({ query: `query{repository(owner:"acme",name:"mono"){pullRequest(number:${pr ?? 0}){number headRefOid}}}` }) });
  step("GraphQL read with the installation token (Cursor: REST and GraphQL)", gql.ok, await gql.json().catch(() => null));
  if (vendor === "cursor") {
    try { const bp = await kit.rest.repos.getBranchProtection({ ...REPO, branch: "main" }); step("branch protection read (administration: read)", bp.status === 200); }
    catch (e: any) { step("branch protection read (administration: read)", false, `${e.status} ${e.message}`); }
  }

  // 5. Webhooks delivered to its URL and signed with its secret.
  await sleep(2500);
  const got = await hooks(app.slug);
  const kinds: Record<string, number> = {}; let bad = 0;
  for (const g of got) { kinds[g.event] = (kinds[g.event] ?? 0) + 1; if (!g.ok) bad++; }
  const want = vendor === "codex" ? ["installation", "push", "pull_request"] : vendor === "ghmcp" ? ["pull_request", "check_run"] : ["pull_request", "check_run"];
  step("webhooks received, signatures valid", want.every((k) => kinds[k]) && bad === 0, { kinds, badSignatures: bad });

  // 6. Revoke (DELETE /installation/token), then PUSHES pushes starting 1 s later, each must be refused.
  const del = await fetch(`${API}/installation/token`, { method: "DELETE", headers: { authorization: `token ${it.token}` } });
  step("DELETE /installation/token", del.status === 204, { status: del.status });
  await sleep(1000);
  let accepted = 0; const texts: Record<string, number> = {};
  for (let i = 0; i < PUSHES; i++) {
    writeFileSync(`${dir}/${vendor}.txt`, `after revoke ${i}\n`);
    await run("git", ["-c", `user.name=${vendor}[bot]`, "-c", `user.email=${vendor}@example.com`, "commit", "-qam", `after revoke ${i}`], { cwd: dir });
    const p = await git(["push", "-q", "URL", `HEAD:refs/heads/${branch}`], dir, it.token);
    if (p.ok) accepted++; else { const k = (p.out.match(/fatal: [^\n]*|remote: [^\n]*/)?.[0] ?? p.out.slice(0, 80)).replace(/x-access-token:[^@]+@/, "x-access-token:***@"); texts[k] = (texts[k] ?? 0) + 1; }
  }
  const restAfter = await kit.rest.pulls.get({ ...REPO, pull_number: pr ?? 1 }).then((x) => x.status, (e) => e.status);
  step(`pushes 1 s after revoke refused ${PUSHES - accepted} of ${PUSHES}`, accepted === 0, { accepted, texts, restReadAfterRevoke: restAfter });
  rmSync(dir, { recursive: true, force: true });

  // 7. github-mcp-server's default toolsets on this App's installation token (a fresh one, since the first is revoked).
  if (vendor === "ghmcp") await ghmcp(auth, inst, branch, head);
  log.ok = (log.steps as { ok: boolean }[]).every((s) => s.ok);
  writeFileSync(`${ROOT}results/raw/wp15-l007-${vendor}${H === URLS.base ? "-base" : ""}${process.argv.includes("--with-checks") ? "-checks" : ""}.json`, JSON.stringify(log, null, 1));
  console.log(vendor, log.ok ? "ALL OK" : "SOME FAILED");
}

async function ghmcp(auth: ReturnType<typeof createAppAuth>, inst: number, branch: string, head: string) {
  const t2 = await auth({ type: "installation", installationId: inst, refresh: true }) as { token: string };
  const host = H.replace("https://", "");
  const tr = new StdioClientTransport({ command: `${process.env.HOME}/.cache/wp15/github-mcp-server`, args: ["stdio"],
    env: { ...process.env, GITHUB_PERSONAL_ACCESS_TOKEN: t2.token, GITHUB_HOST: `https://${host}` } as Record<string, string>, stderr: "ignore" });
  const c = new Client({ name: "wp15-l007", version: "1" });
  await c.connect(tr);
  const tools = (await c.listTools()).tools.map((x) => x.name);
  const o = { owner: "acme", repo: "mono" };
  const calls: [string, Record<string, unknown>][] = [
    ["list_branches", o], ["list_pull_requests", o],
    ["create_pull_request", { ...o, head: branch, base: "main", title: "mcp-bot: change" }],
    ["pull_request_read", { ...o, method: "get_check_runs", pullNumber: 0 }],
    ["get_commit", { ...o, sha: head }],
  ];
  const res: Record<string, unknown> = {};
  let prNum = 0;
  for (const [n, a] of calls) {
    if (n === "pull_request_read") a.pullNumber = prNum;
    try { const r = await c.callTool({ name: n, arguments: a }); const txt = JSON.stringify(r.content).slice(0, 400);
          if (n === "create_pull_request") prNum = (await new Octokit({ baseUrl: API, auth: t2.token }).rest.pulls.list({ ...o, head: `acme:${branch}` })).data.find((x) => x.head.ref === branch)?.number ?? 0;
          res[n] = { isError: !!r.isError, text: txt }; }
    catch (e) { res[n] = { error: String(e).slice(0, 300) }; }
  }
  await c.close();
  const checks = String((res.pull_request_read as { text?: string })?.text ?? "");
  step("github-mcp-server default toolsets on the installation token", tools.length > 0 && !(res.create_pull_request as { isError?: boolean })?.isError, { tools: tools.length, pr: prNum, results: res });
  step("github-mcp-server reads graftyard/overlap (pull_request_read get_check_runs)", checks.includes("graftyard/overlap"), checks.slice(0, 200));
}

await main();
