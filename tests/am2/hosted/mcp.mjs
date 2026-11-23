// Shared by AM2-WP1's hosted drivers (L0-08, A43, LF-6) on a test install: graft-mcp's origin, the CIMD host, the
// scripted browser, the settings page's grant list, and the user's token rows. Environment as
// `tests/am1/hosted/lib.mjs` (GRAFT_EDGE, GRAFT_TOOLS_EDGE, BED_TOKEN_FILE, M3_STATE), plus
//   GRAFT_MCP   graft-mcp's origin (default https://graft-m3f-mcp.<sub>.workers.dev)
//   GRAFT_AUX   the CIMD host, `tests/m0/src/wp15/aux.ts` deployed as `graft-m3f-mcp-aux`
// The user is `m3-member`, signed in to the host by its PAT, as the edge's stand-in `/login/oauth/authorize` takes it.
// No token goes on argv, in a URL this module logs, or in a log line (`scan`).
import { createHash, randomBytes } from "node:crypto";
import { bed, EDGE, secret, state, tokens, tools } from "../../am1/hosted/lib.mjs";

const SUB = process.env.CF_WORKERS_SUBDOMAIN;
export const MCP = process.env.GRAFT_MCP ?? `https://graft-m3f-mcp.${SUB}.workers.dev`;
export const AUX = process.env.GRAFT_AUX ?? `https://graft-m3f-mcp-aux.${SUB}.workers.dev`;
export const MEMBER = tokens.member;
export const CLIENTS = { "ts-v2": 4710, "py-v2": 4711, "go-v18": 4712, "ts-v1": 4713 };
export const cimd = (name) => `${AUX}/cimd/${name}.json`;
export const redirectOf = (name) => `http://127.0.0.1:${CLIENTS[name] ?? 4710}/callback`;
// m3org/j4 (repo:65536) was retired and purged on 2026-11-18; day 2 made a fresh one (AM2_REPO).
export const REPO = process.env.AM2_REPO ?? "m3org/j4";
const b64u = (b) => Buffer.from(b).toString("base64url");
const unent = (s) => s.replace(/&#(\d+);/g, (_, c) => String.fromCharCode(Number(c))).replace(/&quot;/g, "\"").replace(/&amp;/g, "&");
const cookieOf = (r) => (r.headers.get("set-cookie") ?? "").split(";")[0];

/** An authorization request as an SDK client builds it: PKCE S256, CIMD `client_id`, RFC 8707 `resource`. */
export function authorizeUrl({ scope = "write:write", client = "ts-v2", redirect, resource = `${MCP}/`, origin = MCP } = {}) {
  const verifier = b64u(randomBytes(32));
  const u = new URL(`${origin}/authorize`);
  const q = { response_type: "code", client_id: client.startsWith("http") ? client : cimd(client), redirect_uri: redirect ?? redirectOf(client),
    code_challenge: b64u(createHash("sha256").update(verifier).digest()), code_challenge_method: "S256", state: randomBytes(8).toString("hex"), scope, resource };
  for (const [k, v] of Object.entries(q)) u.searchParams.set(k, v);
  return { url: u.toString(), verifier, client_id: q.client_id, redirect_uri: q.redirect_uri, state: q.state };
}

/** The host's stand-in consent: the edge's `GET /login/oauth/authorize`, signed in by the user's PAT. */
async function hostAuthorize(loc, user = MEMBER) {
  const r = await fetch(loc, { redirect: "manual", headers: { authorization: `token ${user}` } });
  const to = r.headers.get("location") ?? "";
  if (r.status !== 302) throw new Error(`host authorize: ${r.status} ${(await r.text()).slice(0, 200)}`);
  return to;
}

/**
 * The user's browser through consent: load the page, submit Allow from graft-mcp's origin with `repos`, follow the
 * host's OAuth, and bring the client its callback URL (`code`, `state`, `iss`). It never reads the client's state.
 */
export async function consent(authUrl, { repos = REPO, decision = "allow", origin = MCP } = {}) {
  const r1 = await fetch(authUrl, { redirect: "manual" });
  const page = await r1.text();
  if (r1.status !== 200) throw new Error(`authorize: ${r1.status} ${page.slice(0, 200)}`);
  const seen = { csp: r1.headers.get("content-security-policy"), xfo: r1.headers.get("x-frame-options") };
  const p = /name=p value="([^"]+)"/.exec(page)?.[1];
  if (!p) throw new Error("no consent form");
  const r2 = await fetch(`${origin}/consent`, { method: "POST", redirect: "manual",
    headers: { "content-type": "application/x-www-form-urlencoded", origin, "sec-fetch-site": "same-origin" },
    body: new URLSearchParams({ p: unent(p), decision, repos }).toString() });
  const loc2 = r2.headers.get("location") ?? "";
  if (decision !== "allow") return { callback: loc2, seen, page };
  if (!loc2.startsWith(EDGE)) throw new Error(`consent: ${r2.status} ${loc2.slice(0, 120)} ${(await r2.text()).slice(0, 200)}`);
  const loc3 = await hostAuthorize(loc2);
  if (!loc3.startsWith(`${origin}/callback`)) throw new Error(`host authorize went to ${loc3.slice(0, 80)}`);
  const r4 = await fetch(loc3, { redirect: "manual", headers: { cookie: cookieOf(r2) } });
  const loc4 = r4.headers.get("location") ?? "";
  if (r4.status !== 302) throw new Error(`callback: ${r4.status} ${(await r4.text()).slice(0, 200)}`);
  return { callback: loc4, seen, page, flowCookie: cookieOf(r2), hostCallback: loc3 };
}

export async function token(form, origin = MCP) {
  const r = await fetch(`${origin}/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(form).toString() });
  const body = await r.json().catch(() => ({}));
  for (const k of ["access_token", "refresh_token"]) if (body[k]) secret(body[k]);
  return { status: r.status, body };
}

/** Consent and the code's redemption, as a client: `{access_token, refresh_token, scope}`. */
export async function login(scope = "write:write", client = "ts-v2", origin = MCP) {
  const a = authorizeUrl({ scope, client, origin, resource: `${origin}/` });
  const { callback } = await consent(a.url, { origin });
  const code = new URL(callback).searchParams.get("code");
  const t = await token({ grant_type: "authorization_code", code, code_verifier: a.verifier, client_id: a.client_id, redirect_uri: a.redirect_uri, resource: `${origin}/` }, origin);
  if (t.status !== 200) throw new Error(`token: ${t.status} ${JSON.stringify(t.body).slice(0, 200)}`);
  return t.body;
}

/** One stateless MCP request with `bearer`: `{status, text}`. */
export async function mcpCall(bearer, method = "tools/list", params = {}, origin = MCP) {
  const r = await fetch(`${origin}/`, { method: "POST", headers: { ...(bearer ? { authorization: `Bearer ${bearer}` } : {}), "content-type": "application/json",
    accept: "application/json, text/event-stream", "mcp-protocol-version": "2026-07-28", "mcp-method": method,
    ...(method === "tools/call" ? { "mcp-name": params.name } : {}) },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params: { ...params, _meta: { "io.modelcontextprotocol/protocolVersion": "2026-07-28",
      "io.modelcontextprotocol/clientInfo": { name: "am2-l008", version: "1" }, "io.modelcontextprotocol/clientCapabilities": {} } } }) });
  return { status: r.status, text: await r.text(), www: r.headers.get("www-authenticate") };
}

/** The user's grants as `https://mcp.<host>/settings` lists them, signed in through the host's OAuth: `[{client, id}]`. */
export async function settingsGrants(origin = MCP) {
  const r1 = await fetch(`${origin}/settings`, { redirect: "manual" });
  const cookie = cookieOf(r1);
  const back = await hostAuthorize(r1.headers.get("location"));
  const r2 = await fetch(back, { redirect: "manual", headers: { cookie } });
  const session = (r2.headers.getSetCookie?.() ?? []).map((c) => c.split(";")[0]).find((c) => c.startsWith("__Host-graft_mcp_session=") && c.length > 30);
  const r3 = await fetch(`${origin}/settings`, { headers: { cookie: session ?? "" } });
  const html = await r3.text();
  return [...html.matchAll(/<li><code>([^<]*)<\/code>[^]*?name=grant value="([^"]+)"/g)].map((m) => ({ client: unent(m[1]), id: m[2] }));
}

/**
 * The user's `Graftyard MCP` user-to-server rows: every credential the `Identity` chain lists under `App <client>`,
 * read on its `Tokens` shard. `unscoped` are the live rows with no grant (the `ghu_` graft-mcp holds sealed), `scoped`
 * the live rows `token/scoped` made.
 */
export async function mcpTokens(account = state.member, client = state.agentApps["graftyard-mcp"].client) {
  const v = (await bed({ chain: `identity:${account}`, read: {} })).value;
  const mine = (v?.credentials ?? []).filter((c) => c.token && c.title === `App ${client}`);
  const rows = await Promise.all(mine.map(async (c) => {
    const shard = Buffer.from(c.token, "hex").readUInt32BE(0) % 3072;
    return (await tools("/read", { chain: `tokens:${shard}`, q: { token: c.token } })).value?.token;
  }));
  const live = rows.filter((r) => r && !r.revoked && r.kind === "UserToServer" && (r.expires == null || r.expires > Date.now()));
  return { listed: mine.length, unscoped: live.filter((r) => !r.fields?.grant).length, scoped: live.filter((r) => r.fields?.grant).length };
}
