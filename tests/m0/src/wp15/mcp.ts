// M0-WP15: `graft-mcp`, deployed as graft-wp15-mcp (wp15/graft-wp15-mcp.jsonc). design/AGENT-LAYER.md, Shape,
// "graft-mcp" (`Consent`, `McpGrant`, `McpToken`, `McpRefresh`, `WriteScope`, `call`) and journey A7. It holds no
// state of its own: grant rows live on the host (`/api/graftyard/v1/mcp/grants`), and every token, code, and cookie is
// sealed. It calls the host only over the `EDGE` service binding.
//
//   POST /                                   MCP (createMcpHandler, spec 2026-07-28; 2025-11-25 on the legacy lane)
//   GET  /.well-known/oauth-protected-resource       RFC 9728
//   GET  /.well-known/oauth-authorization-server     RFC 8414 (PKCE S256, CIMD, RFC 9207 iss)
//   GET  /authorize      CIMD fetch and checks, RFC 8707 resource, then the consent page (frame-ancestors 'none')
//   POST /consent        the consent form: sets the __Host- cookie bound to state, redirects to the host's OAuth
//   GET  /callback       cookie must match state, once; host code -> ghu_; grant row; our code to the client
//   POST /token          authorization_code (PKCE), refresh_token (narrowed, generation), and RFC 8693 exchange
//   GET  /settings, POST /settings/revoke    the user's grants
//
// Prototype simplifications, each named in results/m0-wp15.md: the user's GHES browser session is a cookie carrying a
// harness PAT; a script secret stands in for Secrets Store; the per-IP /authorize limit is per isolate.

import { McpServer } from "@modelcontextprotocol/server";
import { createMcpHandler } from "agents/mcp/server";
import { z } from "zod";
import { open, seal, sha256b64u } from "./seal.ts";

interface Env { EDGE: Fetcher; M0_TOKEN: string; MCP_CLIENT_SECRET: string; HOST_ORIGIN: string; SELF_ORIGIN: string; DENY_HOSTS: string }

export type WriteScope = "Read" | "Write" | "WriteApproveMerge";
export const TOOLSETS = ["context", "repos", "issues", "pull_requests", "checks", "coordination", "provenance"] as const;
export type Toolset = (typeof TOOLSETS)[number];
/** `Consent`: what the page shows and the user approves. */
export interface Consent { client: string; redirect_hosts: string[]; localhost_only: boolean; repos: string[]; tools: Toolset[]; write: WriteScope }
interface Pending extends Consent { redirect_uri: string; state: string | null; challenge: string; resource: string; nonce: string; exp: number }
interface Code extends Consent { sub: number; grant: string; generation: number; redirect_uri: string; challenge: string; resource: string;
  upstream: string; ghr: string; exp: number; once: string }
/** `McpToken`. `upstream` is the scoped ghu_, sealed with the rest. */
export interface McpToken { aud: string; sub: number; client: string; grant: string; repos: string[]; tools: Toolset[]; write: WriteScope; upstream: string; exp: number }
/** `McpRefresh`: seals the narrowing and the grant's generation. */
export interface McpRefresh { sub: number; aud: string; client: string; grant: string; generation: number; repos: string[]; tools: Toolset[];
  write: WriteScope; upstream: string /* ghr_ */; exp: number }

const SCOPE_ORDER: WriteScope[] = ["Read", "Write", "WriteApproveMerge"];
const wireWrite = (w: WriteScope) => ({ Read: "read", Write: "write", WriteApproveMerge: "write_approve_merge" })[w];
const fromScope = (s: string): { write: WriteScope; tools: Toolset[] } | null => {
  // OAuth `scope`: space-separated `write:<scope>` and `tools:<a,b>`. Unknown parts are refused, not dropped.
  let write: WriteScope = "Read"; let tools: Toolset[] = ["context", "repos", "issues", "pull_requests"];
  for (const part of s.split(" ").filter(Boolean)) {
    const [k, v] = part.split(":");
    if (k === "write" && (["read", "write", "write_approve_merge"] as string[]).includes(v)) write = (({ read: "Read", write: "Write", write_approve_merge: "WriteApproveMerge" }) as const)[v as "read"];
    else if (k === "tools" && v.split(",").every((t) => (TOOLSETS as readonly string[]).includes(t))) tools = v.split(",") as Toolset[];
    else if (k === "offline_access") continue;
    else return null;
  }
  return { write, tools };
};
const DOC_HEADERS = { "content-security-policy": "frame-ancestors 'none'; default-src 'none'; style-src 'unsafe-inline'; form-action 'self'",
  "x-frame-options": "DENY", "cache-control": "no-store", "referrer-policy": "no-referrer" };
const jerr = (status: number, error: string, desc: string, h: Record<string, string> = {}) =>
  Response.json({ error, error_description: desc }, { status, headers: { "cache-control": "no-store", ...h } });
const esc = (s: string) => s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);

// ---- Per-isolate counters (prototype; a real deploy adds a WAF rule on mcp.<host>) ----
const authorizeHits = new Map<string, { n: number; t: number }>();
const AUTHORIZE_PER_MIN = 60;
const inflight = new Map<string, number>();
const IN_FLIGHT_MAX = 10;
const cimdCache = new Map<string, { doc: CimdDoc; until: number }>();

interface CimdDoc { client_id: string; client_name?: string; redirect_uris: string[]; grant_types?: string[]; token_endpoint_auth_method?: string }

/** CIMD fetch: 5 s timeout, 5 KB cap, no redirects, the install's own zones denied, cached by Cache-Control. */
export async function fetchCimd(url: string, env: Env): Promise<CimdDoc | string> {
  let u: URL; try { u = new URL(url); } catch { return "client_id is not a URL"; }
  if (u.protocol !== "https:" || u.pathname === "/" || u.hash || u.username) return "client_id must be an https URL with a path";
  if (env.DENY_HOSTS.split(",").some((d) => d && (u.hostname === d || u.hostname.endsWith(`.${d}`)))) return "client_id is on a denied zone";
  const hit = cimdCache.get(url);
  if (hit && hit.until > Date.now()) return hit.doc;
  let r: Response;
  try { r = await fetch(url, { redirect: "manual", signal: AbortSignal.timeout(5_000), headers: { accept: "application/json" } }); }
  catch { return "client metadata fetch failed or timed out"; }
  if (r.status !== 200) return `client metadata fetch returned ${r.status}`;
  const reader = r.body!.getReader(); const parts: Uint8Array[] = []; let n = 0;
  for (;;) { const { done, value } = await reader.read(); if (done) break; n += value.length; if (n > 5_120) { await reader.cancel(); return "client metadata over 5 KB"; } parts.push(value); }
  let doc: CimdDoc;
  try { doc = JSON.parse(new TextDecoder().decode(Uint8Array.from(parts.flatMap((p) => [...p])))); } catch { return "client metadata is not JSON"; }
  if (doc.client_id !== url) return "client metadata client_id does not equal its URL";
  if (!Array.isArray(doc.redirect_uris) || !doc.redirect_uris.length) return "client metadata has no redirect_uris";
  const maxAge = Number(/max-age=(\d+)/.exec(r.headers.get("cache-control") ?? "")?.[1] ?? 0);
  if (maxAge > 0) cimdCache.set(url, { doc, until: Date.now() + Math.min(maxAge, 86_400) * 1000 });
  return doc;
}

const edge = (env: Env, path: string, init: RequestInit = {}) => env.EDGE.fetch(new Request(`${env.HOST_ORIGIN}${path}`, init));
const clientBasic = (env: Env) => `Basic ${btoa(`Iv1.graftyardmcp:${env.MCP_CLIENT_SECRET}`)}`;

/** `token/scoped` with the grant fields, then `DELETE /applications/{client_id}/token` on the unscoped token. */
async function narrow(env: Env, ghu: string, c: { grant: string; write: WriteScope; repos: string[] }): Promise<string | { status: number; body: string }> {
  const r = await edge(env, "/api/v3/applications/Iv1.graftyardmcp/token/scoped", { method: "POST", headers: { authorization: clientBasic(env), "content-type": "application/json" },
    body: JSON.stringify({ access_token: ghu, repositories: c.repos, graftyard_grant: c.grant, graftyard_write: wireWrite(c.write) }) });
  if (r.status !== 200) return { status: r.status, body: await r.text() };
  const scoped = (await r.json<{ token: string }>()).token;
  await edge(env, "/api/v3/applications/Iv1.graftyardmcp/token", { method: "DELETE", headers: { authorization: clientBasic(env), "content-type": "application/json" },
    body: JSON.stringify({ access_token: ghu }) });
  return scoped;
}
async function hostToken(env: Env, form: Record<string, string>): Promise<{ access_token?: string; refresh_token?: string; error?: string }> {
  const r = await edge(env, "/login/oauth/access_token", { method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ client_id: "Iv1.graftyardmcp", client_secret: env.MCP_CLIENT_SECRET, ...form }).toString() });
  return r.json();
}

function consentPage(c: Pending, sealed: string): Response {
  const hosts = c.redirect_hosts.map((h) => `<li><code>${esc(h)}</code></li>`).join("");
  const html = `<!doctype html><meta charset=utf-8><title>Authorize ${esc(new URL(c.client).host)}</title>
<h1>Authorize an MCP client</h1>
<p>Client metadata from <b><code>${esc(new URL(c.client).host)}</code></b> (${esc(c.client)}).</p>
<p>It will redirect to:</p><ul>${hosts}</ul>
${c.localhost_only ? `<p><b>Warning:</b> every redirect goes to this computer (localhost). Only continue if you started this client yourself.</p>` : ""}
<p>Repositories: ${c.repos.map((r) => `<code>${esc(r)}</code>`).join(", ")}<br>Toolsets: ${c.tools.join(", ")}<br>Write scope: <b>${c.write}</b></p>
<form method=post action="/consent"><input type=hidden name=p value="${esc(sealed)}">
<button name=decision value=allow>Allow</button> <button name=decision value=deny>Deny</button></form>`;
  return new Response(html, { headers: { "content-type": "text/html; charset=utf-8", ...DOC_HEADERS } });
}

async function authorize(req: Request, url: URL, env: Env): Promise<Response> {
  const ip = req.headers.get("cf-connecting-ip") ?? "?";
  const h = authorizeHits.get(ip);
  if (h && Date.now() - h.t < 60_000) { if (++h.n > AUTHORIZE_PER_MIN) return new Response("Too many authorization requests", { status: 429, headers: { "retry-after": "60" } }); }
  else authorizeHits.set(ip, { n: 1, t: Date.now() });
  const q = url.searchParams;
  const clientId = q.get("client_id") ?? "";
  const doc = await fetchCimd(clientId, env);
  // Before the redirect URI is trusted, errors are shown, not redirected (RFC 6749 4.1.2.1).
  if (typeof doc === "string") return new Response(`invalid_client: ${doc}`, { status: 400, headers: DOC_HEADERS });
  const redirect = q.get("redirect_uri") ?? "";
  if (!doc.redirect_uris.includes(redirect)) return new Response("invalid_request: redirect_uri is not in the client's metadata", { status: 400, headers: DOC_HEADERS });
  const back = (error: string, desc: string) => { const u = new URL(redirect); u.searchParams.set("error", error); u.searchParams.set("error_description", desc);
    if (q.get("state")) u.searchParams.set("state", q.get("state")!); u.searchParams.set("iss", env.SELF_ORIGIN); return Response.redirect(u.toString(), 302); };
  if (q.get("response_type") !== "code") return back("unsupported_response_type", "only code");
  if (q.get("code_challenge_method") !== "S256" || !/^[A-Za-z0-9_-]{43}$/.test(q.get("code_challenge") ?? "")) return back("invalid_request", "PKCE S256 is required");
  const resource = q.get("resource") ?? "";
  if (resource.replace(/\/$/, "") !== env.SELF_ORIGIN) return back("invalid_target", `resource must be ${env.SELF_ORIGIN}/`);
  const sc = fromScope(q.get("scope") ?? "");
  if (!sc) return back("invalid_scope", "unknown scope");
  const repos = (q.get("repos") ?? "acme/mono").split(",").filter(Boolean).slice(0, 4);
  const hosts = [...new Set(doc.redirect_uris.map((r) => new URL(r).hostname))];
  const pending: Pending = { client: clientId, redirect_hosts: hosts, localhost_only: hosts.every((x) => x === "localhost" || x === "127.0.0.1" || x === "[::1]"),
    repos, tools: sc.tools, write: sc.write, redirect_uri: redirect, state: q.get("state"), challenge: q.get("code_challenge")!, resource: env.SELF_ORIGIN + "/",
    nonce: crypto.randomUUID(), exp: Date.now() + 600_000 };
  return consentPage(pending, await seal(env.M0_TOKEN, "gymcp_pending_", pending));
}

/** The consent POST. A cross-site POST from a framing page carries no Origin of ours, so it is refused; the page
 *  itself cannot be framed (`frame-ancestors 'none'`). */
async function consent(req: Request, env: Env): Promise<Response> {
  const origin = req.headers.get("origin");
  const site = req.headers.get("sec-fetch-site");
  if (origin !== env.SELF_ORIGIN || (site && site !== "same-origin")) return new Response("consent must be submitted from this origin", { status: 403, headers: DOC_HEADERS });
  const f = new URLSearchParams(await req.text());
  const p = await open<Pending>(env.M0_TOKEN, "gymcp_pending_", f.get("p"));
  if (!p || p.exp < Date.now()) return new Response("consent expired", { status: 400, headers: DOC_HEADERS });
  if (f.get("decision") !== "allow") {
    const u = new URL(p.redirect_uri); u.searchParams.set("error", "access_denied"); if (p.state) u.searchParams.set("state", p.state); u.searchParams.set("iss", env.SELF_ORIGIN);
    return Response.redirect(u.toString(), 302);
  }
  // Only now: the signed __Host- cookie bound to our upstream state, and the redirect to the host's OAuth.
  const upstate = crypto.randomUUID();
  const cookie = await seal(env.M0_TOKEN, "gymck_", { upstate, p });
  const to = new URL(`${env.HOST_ORIGIN}/login/oauth/authorize`);
  to.searchParams.set("client_id", "Iv1.graftyardmcp"); to.searchParams.set("redirect_uri", `${env.SELF_ORIGIN}/callback`); to.searchParams.set("state", upstate);
  return new Response(null, { status: 302, headers: { location: to.toString(), "set-cookie": `__Host-graft_mcp=${cookie}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=600`, ...DOC_HEADERS } });
}

const usedCookies = new Set<string>(); // "once" within an isolate; the sealed cookie also expires in 10 min [prototype]

async function callback(req: Request, url: URL, env: Env): Promise<Response> {
  const raw = /(?:^|;\s*)__Host-graft_mcp=([^;]+)/.exec(req.headers.get("cookie") ?? "")?.[1] ?? null;
  const ck = await open<{ upstate: string; p: Pending }>(env.M0_TOKEN, "gymck_", raw);
  if (!ck || ck.upstate !== url.searchParams.get("state") || usedCookies.has(ck.upstate) || ck.p.exp < Date.now())
    return new Response("state does not match a consent from this browser", { status: 400, headers: DOC_HEADERS });
  usedCookies.add(ck.upstate);
  const p = ck.p;
  const t = await hostToken(env, { code: url.searchParams.get("code") ?? "" });
  if (!t.access_token || !t.refresh_token) return new Response(`upstream: ${t.error ?? "no token"}`, { status: 502, headers: DOC_HEADERS });
  const g = await edge(env, "/api/graftyard/v1/mcp/grants", { method: "POST", headers: { authorization: clientBasic(env), "x-graftyard-user-token": t.access_token, "content-type": "application/json" },
    body: JSON.stringify({ client: p.client, redirect_hosts: p.redirect_hosts, write: wireWrite(p.write) }) });
  if (g.status !== 201) return new Response(`grant: ${g.status} ${await g.text()}`, { status: 502, headers: DOC_HEADERS });
  const grant = await g.json<{ id: string; generation: number }>();
  const sub = (await edgeUser(env, t.access_token)) ?? 0;
  const scoped = await narrow(env, t.access_token, { grant: grant.id, write: p.write, repos: p.repos });
  if (typeof scoped !== "string") return new Response(`token/scoped: ${scoped.status} ${scoped.body}`, { status: 502, headers: DOC_HEADERS });
  const code: Code = { client: p.client, redirect_hosts: p.redirect_hosts, localhost_only: p.localhost_only, repos: p.repos, tools: p.tools, write: p.write,
    sub, grant: grant.id, generation: grant.generation, redirect_uri: p.redirect_uri, challenge: p.challenge, resource: p.resource, upstream: scoped,
    ghr: t.refresh_token, exp: Date.now() + 60_000, once: crypto.randomUUID() };
  const u = new URL(p.redirect_uri);
  u.searchParams.set("code", await seal(env.M0_TOKEN, "gymcc_", code));
  if (p.state) u.searchParams.set("state", p.state);
  u.searchParams.set("iss", env.SELF_ORIGIN);
  return new Response(null, { status: 302, headers: { location: u.toString(), "set-cookie": "__Host-graft_mcp=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0" } });
}
/** The user id behind a token, from the token's own claims as the host's `GET /user` would return it. */
async function edgeUser(env: Env, tok: string): Promise<number | null> {
  const r = await edge(env, "/api/v3/user", { headers: { authorization: `Bearer ${tok}` } });
  return r.ok ? (await r.json<{ id: number }>()).id : null;
}

const usedCodes = new Set<string>();

async function tokenEndpoint(req: Request, env: Env): Promise<Response> {
  const f = Object.fromEntries(new URLSearchParams(await req.text()));
  const aud = env.SELF_ORIGIN + "/";
  const issue = async (t: Omit<McpToken, "aud" | "exp">, r: Omit<McpRefresh, "aud" | "exp">) => Response.json({
    access_token: await seal(env.M0_TOKEN, "gymcp_", { ...t, aud, exp: Date.now() + 3_600_000 } satisfies McpToken), token_type: "Bearer", expires_in: 3600,
    refresh_token: await seal(env.M0_TOKEN, "gymcr_", { ...r, aud, exp: Date.now() + 30 * 86_400_000 } satisfies McpRefresh),
    scope: `write:${wireWrite(t.write)} tools:${t.tools.join(",")}` }, { headers: { "cache-control": "no-store" } });
  if (f.grant_type === "authorization_code") {
    const c = await open<Code>(env.M0_TOKEN, "gymcc_", f.code);
    if (!c || c.exp < Date.now() || usedCodes.has(c.once)) return jerr(400, "invalid_grant", "code is invalid, expired, or used");
    if (f.client_id !== c.client) return jerr(400, "invalid_grant", "code was issued to another client");
    if (f.redirect_uri !== c.redirect_uri) return jerr(400, "invalid_grant", "redirect_uri differs from the authorization request");
    if (!f.code_verifier || await sha256b64u(f.code_verifier) !== c.challenge) return jerr(400, "invalid_grant", "PKCE verification failed");
    if (f.resource && f.resource.replace(/\/$/, "") !== env.SELF_ORIGIN) return jerr(400, "invalid_target", "resource mismatch");
    usedCodes.add(c.once);
    const base = { sub: c.sub, client: c.client, grant: c.grant, repos: c.repos, tools: c.tools, write: c.write };
    return issue({ ...base, upstream: c.upstream }, { ...base, generation: c.generation, upstream: c.ghr });
  }
  if (f.grant_type === "refresh_token") {
    const r = await open<McpRefresh>(env.M0_TOKEN, "gymcr_", f.refresh_token);
    if (!r || r.exp < Date.now() || r.aud !== aud) return jerr(400, "invalid_grant", "refresh token is invalid");
    if (f.client_id && f.client_id !== r.client) return jerr(400, "invalid_grant", "refresh token was issued to another client");
    // A refresh can only narrow: a requested scope wider than the grant is refused, never widened.
    if (f.scope) {
      const want = fromScope(f.scope);
      if (!want || SCOPE_ORDER.indexOf(want.write) > SCOPE_ORDER.indexOf(r.write) || want.tools.some((t) => !r.tools.includes(t)))
        return jerr(400, "invalid_scope", "a refresh cannot widen its grant");
    }
    const gen = await edge(env, `/api/graftyard/v1/mcp/grants/${r.grant}/generation`, { method: "POST", headers: { authorization: clientBasic(env), "content-type": "application/json" },
      body: JSON.stringify({ generation: r.generation }) });
    if (gen.status === 409) return jerr(400, "invalid_grant", "this refresh token was already used, so the grant is revoked");
    if (gen.status !== 200) return jerr(502, "temporarily_unavailable", `grant generation: ${gen.status}`);
    const g = await gen.json<{ generation: number }>();
    const t = await hostToken(env, { grant_type: "refresh_token", refresh_token: r.upstream });
    if (!t.access_token || !t.refresh_token) return jerr(400, "invalid_grant", `upstream refresh: ${t.error ?? "failed"}`);
    const want = f.scope ? fromScope(f.scope)! : { write: r.write, tools: r.tools };
    const scoped = await narrow(env, t.access_token, { grant: r.grant, write: want.write, repos: r.repos });
    if (typeof scoped !== "string") return jerr(400, "invalid_grant", `token/scoped: ${scoped.status}`);
    const base = { sub: r.sub, client: r.client, grant: r.grant, repos: r.repos, tools: want.tools, write: want.write };
    return issue({ ...base, upstream: scoped }, { ...base, generation: g.generation, upstream: t.refresh_token });
  }
  if (f.grant_type === "urn:ietf:params:oauth:grant-type:token-exchange") {
    // RFC 8693: an McpToken for a narrowed host token (A7, research/agents.md R13). The result is the scoped ghu_
    // re-scoped to a subset; graft-mcp hands it to sandboxed code or a sub-agent that must call REST directly.
    const t = await open<McpToken>(env.M0_TOKEN, "gymcp_", f.subject_token);
    if (!t || t.exp < Date.now() || t.aud !== aud || f.subject_token_type !== "urn:ietf:params:oauth:token-type:access_token")
      return jerr(400, "invalid_grant", "subject_token is not a live access token for this server");
    const want = fromScope(f.scope ?? `write:${wireWrite(t.write)}`);
    const repos = (f.repos ?? t.repos.join(",")).split(",").filter(Boolean);
    if (!want || SCOPE_ORDER.indexOf(want.write) > SCOPE_ORDER.indexOf(t.write) || repos.some((x) => !t.repos.includes(x)))
      return jerr(400, "invalid_scope", "an exchanged token cannot be wider than its subject");
    const r = await edge(env, "/api/v3/applications/Iv1.graftyardmcp/token/scoped", { method: "POST", headers: { authorization: clientBasic(env), "content-type": "application/json" },
      body: JSON.stringify({ access_token: t.upstream, repositories: repos, graftyard_grant: t.grant, graftyard_write: wireWrite(want.write) }) });
    if (r.status !== 200) return jerr(400, "invalid_grant", `token/scoped: ${r.status}`);
    return Response.json({ access_token: (await r.json<{ token: string }>()).token, issued_token_type: "urn:ietf:params:oauth:token-type:access_token",
      token_type: "Bearer", expires_in: 3600, scope: `write:${wireWrite(want.write)}` }, { headers: { "cache-control": "no-store" } });
  }
  return jerr(400, "unsupported_grant_type", String(f.grant_type));
}

// ---- Tools: `call`. Every tool is one to three REST calls through the edge with `upstream`. ----
export interface ToolError { status: number; message: string; retry_after_s: number | null; rate: { remaining: string | null; reset: string | null } | null }
interface Meter { edge: number; n: number }
async function call(env: Env, tok: McpToken, method: string, path: string, body?: unknown, m?: Meter): Promise<unknown> {
  const e0 = Date.now();
  const r = await edge(env, path, { method, headers: { authorization: `Bearer ${tok.upstream}`, accept: "application/vnd.graftyard.agent+json",
    "content-type": "application/json", "user-agent": "graft-mcp" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text();
  if (m) { m.edge += Date.now() - e0; m.n++; }
  let j: unknown = text; try { j = JSON.parse(text); } catch { /* text */ }
  if (r.status >= 400) {
    const e: ToolError = { status: r.status, message: (j as { message?: string })?.message ?? text.slice(0, 200), retry_after_s: r.headers.has("retry-after") ? Number(r.headers.get("retry-after")) : null,
      rate: r.headers.has("x-ratelimit-remaining") ? { remaining: r.headers.get("x-ratelimit-remaining"), reset: r.headers.get("x-ratelimit-reset") } : null };
    throw Object.assign(new Error(e.message), { toolError: e });
  }
  return j;
}
const ok = (v: unknown) => ({ content: [{ type: "text" as const, text: JSON.stringify(v) }] });
const fail = (e: unknown) => ({ isError: true, content: [{ type: "text" as const, text: JSON.stringify((e as { toolError?: ToolError }).toolError ?? { status: 500, message: String(e) }) }] });

function server(env: Env, tok: McpToken, m: Meter): McpServer {
  const callM = (method: string, path: string, b?: unknown) => call(env, tok, method, path, b, m);
  const s = new McpServer({ name: "graft-mcp", version: "0.15.0" });
  const repo = z.object({ owner: z.string(), repo: z.string() });
  const own = (a: { owner: string; repo: string }) => { if (!tok.repos.includes(`${a.owner}/${a.repo}`)) throw Object.assign(new Error("repository not in this grant"), { toolError: { status: 404, message: "repository not in this grant", retry_after_s: null, rate: null } }); return `/api/v3/repos/${a.owner}/${a.repo}`; };
  const tool = (name: string, set: Toolset, desc: string, schema: z.ZodObject, f: (a: any) => Promise<unknown>) => {
    if (!tok.tools.includes(set)) return;
    s.registerTool(name, { description: desc, inputSchema: schema }, async (a: unknown) => { try { return ok(await f(a)); } catch (e) { return fail(e); } });
  };
  tool("get_me", "context", "The signed-in user", z.object({}), () => callM("GET", "/api/v3/user"));
  tool("get_issue", "issues", "Read an issue (trust view applied)", repo.extend({ number: z.number() }), (a) => callM("GET", `${own(a)}/issues/${a.number}`));
  tool("list_issue_comments", "issues", "Comments on an issue", repo.extend({ number: z.number() }), (a) => callM("GET", `${own(a)}/issues/${a.number}/comments`));
  tool("get_ref", "repos", "Read a ref", repo.extend({ ref: z.string() }), (a) => callM("GET", `${own(a)}/git/ref/${a.ref}`));
  tool("get_pull_request", "pull_requests", "Read a pull request", repo.extend({ number: z.number() }), (a) => callM("GET", `${own(a)}/pulls/${a.number}`));
  if (tok.write !== "Read") {
    tool("add_issue_comment", "issues", "Comment on an issue", repo.extend({ number: z.number(), body: z.string() }), (a) => callM("POST", `${own(a)}/issues/${a.number}/comments`, { body: a.body }));
    tool("create_pull_request", "pull_requests", "Open a pull request", repo.extend({ head: z.string(), base: z.string(), title: z.string(), draft: z.boolean().optional() }),
      (a) => callM("POST", `${own(a)}/pulls`, { head: a.head, base: a.base, title: a.title, draft: a.draft ?? true }));
  }
  if (tok.write === "WriteApproveMerge") {
    tool("approve_pull_request", "pull_requests", "Approve a pull request", repo.extend({ number: z.number() }), (a) => callM("POST", `${own(a)}/pulls/${a.number}/reviews`, { event: "APPROVE" }));
    tool("merge_pull_request", "pull_requests", "Merge a pull request", repo.extend({ number: z.number(), sha: z.string() }), (a) => callM("PUT", `${own(a)}/pulls/${a.number}/merge`, { sha: a.sha }));
  }
  tool("get_check_run", "checks", "Read a check run", repo.extend({ id: z.number() }), (a) => callM("GET", `${own(a)}/check-runs/${a.id}`));
  return s;
}

const handlers = new Map<string, ReturnType<typeof createMcpHandler>>();

async function mcp(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const prm = `${env.SELF_ORIGIN}/.well-known/oauth-protected-resource`;
  const unauth = (desc: string) => new Response(JSON.stringify({ error: "invalid_token", error_description: desc }), { status: 401,
    headers: { "content-type": "application/json", "www-authenticate": `Bearer resource_metadata="${prm}", error="invalid_token", error_description="${desc}"` } });
  const bearer = /^Bearer (.+)$/i.exec(req.headers.get("authorization") ?? "")?.[1];
  if (!bearer) return new Response(null, { status: 401, headers: { "www-authenticate": `Bearer resource_metadata="${prm}"` } });
  const tok = await open<McpToken>(env.M0_TOKEN, "gymcp_", bearer);
  // RFC 8707 / MCP: the token must name this server as its audience.
  if (!tok) return unauth("not a token issued by this server");
  if (tok.aud !== env.SELF_ORIGIN + "/") return unauth("token audience is not this server");
  if (tok.exp < Date.now()) return unauth("token expired");
  const n = inflight.get(bearer) ?? 0;
  if (n >= IN_FLIGHT_MAX) return new Response(JSON.stringify({ error: "too many calls in flight for this token" }), { status: 429, headers: { "retry-after": "1", "content-type": "application/json" } });
  inflight.set(bearer, n + 1);
  const t0 = Date.now();
  try {
    const m: Meter = { edge: 0, n: 0 };
    const h = createMcpHandler(() => server(env, tok, m), { route: "/", allowedHostnames: [new URL(env.SELF_ORIGIN).hostname], responseMode: "json" });
    void handlers;
    const r = await h(req, env, ctx);
    const body = await r.arrayBuffer();
    const out = new Response(body, r);
    // mcp = whole request in graft-mcp, edge = time inside the REST calls it made; overhead = mcp - edge.
    out.headers.set("server-timing", `mcp;dur=${Date.now() - t0}, edge;dur=${m.edge}, rest;desc="${m.n}"`);
    return out;
  } finally { inflight.set(bearer, (inflight.get(bearer) ?? 1) - 1); }
}

async function settings(req: Request, url: URL, env: Env): Promise<Response> {
  // The user signs in to settings with their host token (prototype: a bearer ghu_ of Graftyard MCP in a cookie or header).
  const ut = req.headers.get("x-graftyard-user-token") ?? /(?:^|;\s*)graft_user=([^;]+)/.exec(req.headers.get("cookie") ?? "")?.[1] ?? "";
  const h = { authorization: clientBasic(env), "x-graftyard-user-token": ut };
  if (req.method === "POST" && url.pathname === "/settings/revoke") {
    if (req.headers.get("origin") !== env.SELF_ORIGIN) return new Response("forbidden", { status: 403, headers: DOC_HEADERS });
    const id = new URLSearchParams(await req.text()).get("grant") ?? "";
    const r = await edge(env, `/api/graftyard/v1/mcp/grants/${encodeURIComponent(id)}`, { method: "DELETE", headers: h });
    return new Response(r.status === 204 ? "revoked" : `failed: ${r.status}`, { status: r.status === 204 ? 200 : r.status, headers: DOC_HEADERS });
  }
  const r = await edge(env, "/api/graftyard/v1/mcp/grants", { headers: h });
  if (!r.ok) return new Response(`sign in: ${r.status}`, { status: r.status, headers: DOC_HEADERS });
  const rows = await r.json<{ id: string; client: string; write: string; generation: number }[]>();
  return new Response(`<!doctype html><title>MCP grants</title><ul>${rows.map((g) => `<li>${esc(g.client)} (${g.write}, gen ${g.generation})
<form method=post action="/settings/revoke"><input type=hidden name=grant value="${esc(g.id)}"><button>Revoke</button></form></li>`).join("")}</ul>`,
    { headers: { "content-type": "text/html; charset=utf-8", ...DOC_HEADERS } });
}

export default {
  async fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(req.url);
    const p = url.pathname;
    if (p === "/.well-known/oauth-protected-resource" || p === "/.well-known/oauth-protected-resource/")
      return Response.json({ resource: env.SELF_ORIGIN + "/", authorization_servers: [env.SELF_ORIGIN], bearer_methods_supported: ["header"],
        // Only the least-privilege default: SDK clients request every scope listed here (finding, results L0-08).
        scopes_supported: ["write:read"], resource_name: "Graftyard MCP" });
    if (p === "/.well-known/oauth-authorization-server" || p === "/.well-known/openid-configuration")
      return Response.json({ issuer: env.SELF_ORIGIN, authorization_endpoint: `${env.SELF_ORIGIN}/authorize`, token_endpoint: `${env.SELF_ORIGIN}/token`,
        response_types_supported: ["code"], grant_types_supported: ["authorization_code", "refresh_token", "urn:ietf:params:oauth:grant-type:token-exchange"],
        code_challenge_methods_supported: ["S256"], token_endpoint_auth_methods_supported: ["none"], client_id_metadata_document_supported: true,
        authorization_response_iss_parameter_supported: true, scopes_supported: ["write:read", "write:write", "write:write_approve_merge"] });
    if (p === "/authorize" && req.method === "GET") return authorize(req, url, env);
    if (p === "/consent" && req.method === "POST") return consent(req, env);
    if (p === "/callback" && req.method === "GET") return callback(req, url, env);
    if (p === "/token" && req.method === "POST") return tokenEndpoint(req, env);
    if (p === "/settings" || p === "/settings/revoke") return settings(req, url, env);
    if (p === "/") return mcp(req, env, ctx);
    return new Response("Not Found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
