// M0-WP15: the GHES 3.22 user-token surface the `Graftyard MCP` App uses (design/AGENT-LAYER.md, A7 steps 4 to 7), and
// the grant endpoints under /api/graftyard/v1/mcp/grants ("API reference"). A prototype of GHES's behavior, not GHES:
//   GET  /login/oauth/authorize            the user's browser session is the `user_session` cookie (a harness PAT)
//   POST /login/oauth/access_token         code -> ghu_ + ghr_ (8 h, ~6 months), refresh_token -> rotated pair
//   DELETE /api/v3/applications/{cid}/token   one token; the ghr_ is a separate row and survives (A4 is a GHES fact this
//                                             prototype cannot settle; it models "survives")
//   DELETE /api/v3/applications/{cid}/grant   every token, refresh token, and McpGrant of that user
//   DELETE /api/v3/installation/token         revokes a stateless installation token by its jti
// Prototype tokens are `gym0_` (token.ts); a real install mints `ghu_` and `ghs_`.

import type { Host } from "./host.ts";
import { derive, mint, tokenOf, verify } from "./token.ts";
import { APP, MCP_CLIENT } from "./fixture.ts";
import type { Credential, WriteScope } from "./model.ts";

export interface OAuthState {
  codes: Record<string, { user: number; client: string; redirect: string; exp: number; used: boolean }>;
  /** Unscoped user-to-server tokens (ghu_) by jti. Scoped ones are `authz.tokens` rows. */
  users: Record<string, { user: number; app: number; at: number }>;
  /** Refresh tokens (ghr_), one use each: a refresh rotates the pair. */
  refresh: Record<string, { user: number; app: number; at: number }>;
  /** Revoked stateless installation tokens: jti -> exp. */
  revoked: Record<string, number>;
  /** OAuth Apps' and GitHub Apps' user-auth clients: client id -> registered callback URL prefix and App id. */
  clients: Record<string, { app: number; callback: string; secretLabel: string }>;
}
export const freshOAuth = (): OAuthState => ({ codes: {}, users: {}, refresh: {}, revoked: {}, clients: {} });

const DOC = "https://docs.github.com/enterprise-server@3.22/rest";
const json = (status: number, body: unknown, h: Record<string, string> = {}) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json; charset=utf-8", ...h } });
const rand = (n = 20) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");
const cookie = (req: Request, name: string) => new RegExp(`(?:^|;\\s*)${name}=([^;]+)`).exec(req.headers.get("cookie") ?? "")?.[1] ?? null;

/** Whether a verified credential is still live: user tokens need their row, installation tokens must not be revoked. */
export function tokenLive(h: Host, cred: Credential): boolean {
  if (cred.t === "UserToServer" && cred.jti) return !!(h.authz.tokens?.[cred.jti] || h.oauth.users[cred.jti]);
  if (cred.t === "Installation" && cred.jti) return h.oauth.revoked[cred.jti] == null;
  return true;
}

async function clientAuth(h: Host, req: Request, body: Record<string, string>): Promise<string | null> {
  const basic = /^Basic (.+)$/i.exec(req.headers.get("authorization") ?? "");
  const [id, secret] = basic ? atob(basic[1]).split(":") : [body.client_id ?? "", body.client_secret ?? ""];
  const c = h.oauth.clients[id];
  return c && secret === await derive(h.secret, c.secretLabel) ? id : null;
}
async function readForm(req: Request): Promise<Record<string, string>> {
  const t = await req.text();
  if ((req.headers.get("content-type") ?? "").includes("json")) { try { return JSON.parse(t); } catch { return {}; } }
  return Object.fromEntries(new URLSearchParams(t));
}

async function issuePair(h: Host, user: number, app: number): Promise<{ access_token: string; refresh_token: string }> {
  const jti = `u${rand(8)}`;
  h.oauth.users[jti] = { user, app, at: Date.now() };
  const ghr = `ghr_${rand(18)}`;
  h.oauth.refresh[ghr] = { user, app, at: Date.now() };
  return { access_token: await mint(h.secret, { t: "UserToServer", user, app, write: null, jti }), refresh_token: ghr };
}

export async function oauthRoutes(h: Host, req: Request, url: URL): Promise<Response | null> {
  const p = url.pathname, method = req.method;
  let x: RegExpExecArray | null;
  if (method === "GET" && p === "/login/oauth/authorize") {
    const cid = url.searchParams.get("client_id") ?? "", redirect = url.searchParams.get("redirect_uri") ?? "";
    const c = h.oauth.clients[cid];
    if (!c || !redirect.startsWith(c.callback)) return new Response("The redirect_uri MUST match the registered callback URL for this application.", { status: 400 });
    const sess = cookie(req, "user_session");
    const cred = sess ? await verify(h.secret, decodeURIComponent(sess)) : null;
    if (!cred || cred.t !== "Pat") return new Response(null, { status: 302, headers: { location: `/login?return_to=${encodeURIComponent(p + url.search)}` } });
    const code = rand(10);
    h.oauth.codes[code] = { user: cred.user, client: cid, redirect, exp: Date.now() + 600_000, used: false };
    h.saveOAuth();
    const to = new URL(redirect);
    to.searchParams.set("code", code);
    if (url.searchParams.has("state")) to.searchParams.set("state", url.searchParams.get("state")!);
    return new Response(null, { status: 302, headers: { location: to.toString() } });
  }
  if (method === "POST" && p === "/login/oauth/access_token") {
    const b = await readForm(req);
    const cid = await clientAuth(h, req, b);
    const out = (o: Record<string, unknown>) => (req.headers.get("accept") ?? "").includes("json")
      ? json(200, o) : new Response(new URLSearchParams(o as Record<string, string>).toString(), { headers: { "content-type": "application/x-www-form-urlencoded" } });
    if (!cid) return out({ error: "incorrect_client_credentials", error_description: "The client_id and/or client_secret passed are incorrect." });
    const app = h.oauth.clients[cid].app;
    let user: number;
    if (b.grant_type === "refresh_token") {
      const r = h.oauth.refresh[b.refresh_token ?? ""];
      if (!r || r.app !== app) return out({ error: "bad_refresh_token", error_description: "The refresh token passed is incorrect or expired." });
      delete h.oauth.refresh[b.refresh_token];
      user = r.user;
    } else {
      const c = h.oauth.codes[b.code ?? ""];
      if (!c || c.used || c.client !== cid || c.exp < Date.now()) return out({ error: "bad_verification_code", error_description: "The code passed is incorrect or expired." });
      c.used = true;
      user = c.user;
    }
    const pair = await issuePair(h, user, app);
    h.saveOAuth();
    return out({ ...pair, expires_in: 28800, refresh_token_expires_in: 15897600, token_type: "bearer", scope: "" });
  }
  if (method === "DELETE" && (x = /^\/api\/v3\/applications\/([^/]+)\/(token|grant)$/.exec(p))) {
    const b = await readForm(req);
    const cid = await clientAuth(h, req, {});
    if (!cid || cid !== x[1]) return json(401, { message: "Bad credentials", documentation_url: DOC });
    const cred = typeof b.access_token === "string" ? await verify(h.secret, b.access_token) : null;
    if (!cred || cred.t !== "UserToServer" || !cred.jti || !tokenLive(h, cred)) return json(404, { message: "Not Found", documentation_url: DOC });
    if (x[2] === "token") { delete h.oauth.users[cred.jti]; if (h.authz.tokens) delete h.authz.tokens[cred.jti]; }
    else revokeUserApp(h, cred.user, h.oauth.clients[cid].app);
    h.saveOAuth();
    return new Response(null, { status: 204 });
  }
  if (method === "DELETE" && p === "/api/v3/installation/token") {
    const tok = tokenOf(req);
    const cred = tok ? await verify(h.secret, tok) : null;
    if (!cred || cred.t !== "Installation" || !cred.jti || !tokenLive(h, cred)) return json(401, { message: "Bad credentials", documentation_url: DOC });
    h.oauth.revoked[cred.jti] = cred.exp ?? Date.now() + 3_600_000;
    h.saveOAuth();
    return new Response(null, { status: 204 });
  }
  if (p.startsWith("/api/graftyard/v1/mcp/grants")) return h.layer ? grants(h, req, p) : json(404, { message: "Not Found", documentation_url: DOC });
  return null;
}

function revokeUserApp(h: Host, user: number, app: number): void {
  for (const [k, v] of Object.entries(h.oauth.users)) if (v.user === user && v.app === app) delete h.oauth.users[k];
  for (const [k, v] of Object.entries(h.oauth.refresh)) if (v.user === user && v.app === app) delete h.oauth.refresh[k];
  if (app === APP.mcp) {
    for (const [k, v] of Object.entries(h.authz.tokens ?? {})) if (v.user === user) delete h.authz.tokens![k];
    for (const g of Object.values(h.authz.grants ?? {})) if (g.user === user) g.revoked = true;
  }
}
/** Revokes one McpGrant: the row and every scoped token minted under it. */
function revokeGrant(h: Host, id: string): void {
  const g = h.authz.grants?.[id];
  if (g) g.revoked = true;
  for (const [k, v] of Object.entries(h.authz.tokens ?? {})) if (v.grant === id) delete h.authz.tokens![k];
}

/** `/api/graftyard/v1/mcp/grants`: Basic credentials of the install's Graftyard MCP, and the user's ghu_ in
 *  `X-Graftyard-User-Token`. The generation check takes the client credentials alone (see results, design edit). */
async function grants(h: Host, req: Request, p: string): Promise<Response> {
  const basic = /^Basic (.+)$/i.exec(req.headers.get("authorization") ?? "");
  if (!basic || atob(basic[1]) !== `${MCP_CLIENT.id}:${await derive(h.secret, MCP_CLIENT.secretLabel)}`)
    return json(401, { message: "Requires the Graftyard MCP client credentials", documentation_url: DOC });
  const ut = req.headers.get("x-graftyard-user-token");
  const uc = ut ? await verify(h.secret, ut) : null;
  const user = uc && uc.t === "UserToServer" && uc.app === APP.mcp && tokenLive(h, uc) ? uc.user : null;
  const rows = (h.authz.grants ??= {});
  const view = (id: string) => { const g = rows[id]; return { id, client: g.client ?? null, redirect_hosts: g.redirect_hosts ?? [], write: g.write,
    generation: g.generation ?? 0, revoked: !!g.revoked, created_at: g.created_at ?? null }; };
  const b = req.method === "POST" ? await req.json().catch(() => ({})) as Record<string, any> : {};
  let x: RegExpExecArray | null;
  if (p === "/api/graftyard/v1/mcp/grants") {
    if (user == null) return json(401, { message: "X-Graftyard-User-Token must be a live user token of Graftyard MCP", documentation_url: DOC });
    if (req.method === "GET") return json(200, Object.keys(rows).filter((k) => rows[k].user === user && !rows[k].revoked).map(view));
    if (req.method === "POST") {
      const write = ({ read: "Read", write: "Write", write_approve_merge: "WriteApproveMerge" } as Record<string, WriteScope>)[String(b.write ?? "read")];
      if (typeof b.client !== "string" || !Array.isArray(b.redirect_hosts) || !write)
        return json(422, { message: "Validation Failed", documentation_url: DOC, errors: [{ resource: "Grant", field: "client", code: "missing_field" }] });
      const id = `g${rand(8)}`;
      rows[id] = { user, write, client: b.client, redirect_hosts: b.redirect_hosts.map(String), generation: 0, created_at: new Date().toISOString() };
      h.saveOAuth();
      return json(201, view(id));
    }
  }
  if ((x = /^\/api\/graftyard\/v1\/mcp\/grants\/([^/]+)\/generation$/.exec(p)) && req.method === "POST") {
    const g = rows[x[1]];
    if (!g) return json(404, { message: "Not Found", documentation_url: DOC });
    if (user != null && user !== g.user) return json(404, { message: "Not Found", documentation_url: DOC });
    if (g.revoked) return json(409, { message: "This grant is revoked", documentation_url: DOC, revoked: true });
    if (b.generation !== (g.generation ?? 0)) {
      revokeGrant(h, x[1]);
      h.saveOAuth();
      return json(409, { message: "A refresh with an older generation revoked this grant", documentation_url: DOC, revoked: true });
    }
    g.generation = (g.generation ?? 0) + 1;
    h.saveOAuth();
    return json(200, view(x[1]));
  }
  if ((x = /^\/api\/graftyard\/v1\/mcp\/grants\/([^/]+)$/.exec(p)) && req.method === "DELETE") {
    const g = rows[x[1]];
    if (!g || user == null || g.user !== user) return json(404, { message: "Not Found", documentation_url: DOC });
    revokeGrant(h, x[1]);
    h.saveOAuth();
    return new Response(null, { status: 204 });
  }
  return json(404, { message: "Not Found", documentation_url: DOC });
}
