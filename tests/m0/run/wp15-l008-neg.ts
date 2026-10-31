// M0-WP15, L0-08 negative cases against graft-wp15-mcp. Writes results/raw/wp15-l008-neg.json.
//   node run/wp15-l008-neg.ts
// The authorization flow here is the one a client runs (PKCE S256, CIMD `client_id`, `resource`), scripted with
// fetch so each case can be repeated 100 times. Cases:
//   aud      100 tokens with another audience, 100 sealed by another server's key, 100 host tokens, 100 expired: 401
//   refresh  100 refreshes: each keeps its scope and repositories; 100 asking for a wider scope: refused
//   replay   reuse of an already-used refresh token: refused, and the grant is revoked (the newest refresh then fails)
//   consent  /authorize framed (headers), /consent cross-site POST, /consent without Origin, callback without the
//            cookie, callback with another flow's cookie, code redeemed without consent: no code issued
//   8693     exchange to a narrowed host token; wider exchange refused; the narrowed token's REST scope
//   cimd     big, redirect, wrong-id, slow, http, denied zone: no consent page
//   ghr      the host's ghr_ refresh after DELETE of the unscoped ghu_ (prototype model only; A4 needs real GHES)

import { writeFileSync } from "node:fs";
import { createHash, randomBytes } from "node:crypto";
import { seal } from "../src/wp15/seal.ts";
import { consentAndCallback } from "./wp15-browser.ts";
import { ROOT, TOKEN, URLS, tok } from "./wp15.ts";

const b64u = (b: Buffer) => b.toString("base64url");
const CIMD = `${URLS.aux}/cimd/ts-v2.json`, REDIRECT = "http://127.0.0.1:4710/callback";
const res: Record<string, unknown> = { at: new Date().toISOString() };

async function authorizeUrl(scope = "write:write", client = CIMD, redirect = REDIRECT, resource = `${URLS.mcp}/`) {
  const verifier = b64u(randomBytes(32));
  const u = new URL(`${URLS.mcp}/authorize`);
  for (const [k, v] of Object.entries({ response_type: "code", client_id: client, redirect_uri: redirect, code_challenge: b64u(createHash("sha256").update(verifier).digest()),
    code_challenge_method: "S256", state: crypto.randomUUID(), scope, resource })) u.searchParams.set(k, v);
  return { url: u.toString(), verifier };
}
async function token(form: Record<string, string>) {
  const r = await fetch(`${URLS.mcp}/token`, { method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(form).toString() });
  return { status: r.status, body: await r.json() as Record<string, any> };
}
async function login(scope = "write:write") {
  const a = await authorizeUrl(scope);
  const { callback } = await consentAndCallback(a.url);
  const code = new URL(callback).searchParams.get("code")!;
  const t = await token({ grant_type: "authorization_code", code, code_verifier: a.verifier, client_id: CIMD, redirect_uri: REDIRECT, resource: `${URLS.mcp}/` });
  if (t.status !== 200) throw new Error(`token: ${JSON.stringify(t)}`);
  return t.body as { access_token: string; refresh_token: string; scope: string };
}
/** One MCP request with `tok`: the HTTP status of a stateless tools/list. */
async function mcpStatus(bearer: string): Promise<number> {
  const r = await fetch(`${URLS.mcp}/`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json", accept: "application/json, text/event-stream",
    "mcp-protocol-version": "2026-07-28", "mcp-method": "tools/list" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: { _meta: {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": { name: "neg", version: "1" }, "io.modelcontextprotocol/clientCapabilities": {} } } }) });
  await r.arrayBuffer();
  return r.status;
}
async function tools(bearer: string): Promise<string[]> {
  const r = await fetch(`${URLS.mcp}/`, { method: "POST", headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json", accept: "application/json, text/event-stream",
    "mcp-protocol-version": "2026-07-28", "mcp-method": "tools/list" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list", params: { _meta: {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": { name: "neg", version: "1" }, "io.modelcontextprotocol/clientCapabilities": {} } } }) });
  const t = await r.text();
  const j = JSON.parse(t.startsWith("{") ? t : t.split("\n").find((l) => l.startsWith("data:"))!.slice(5));
  return (j.result?.tools ?? []).map((x: { name: string }) => x.name).sort();
}
const count = (xs: number[], f: (s: number) => boolean) => xs.filter(f).length;

// ---- aud ----
{
  const good = await login("write:read");
  const claims = JSON.parse(JSON.stringify({ sub: 11, client: CIMD, grant: "g-none", repos: ["acme/mono"], tools: ["context"], write: "Read", upstream: "x", exp: Date.now() + 3_600_000 }));
  const wrongAud: number[] = [], foreignKey: number[] = [], hostTok: number[] = [], expired: number[] = [];
  for (let i = 0; i < 100; i++) {
    // Sealed with graft-mcp's own key but naming another audience: the forged-aud case a key leak would allow.
    wrongAud.push(await mcpStatus(await seal(TOKEN, "gymcp_", { ...claims, aud: i % 2 ? "https://other-mcp.example/" : `${URLS.host}/` })));
    foreignKey.push(await mcpStatus(await seal(`other-${i}`, "gymcp_", { ...claims, aud: `${URLS.mcp}/` })));
    hostTok.push(await mcpStatus(await tok("bob")));
    expired.push(await mcpStatus(await seal(TOKEN, "gymcp_", { ...claims, aud: `${URLS.mcp}/`, exp: Date.now() - 1000 })));
  }
  res.aud = { good: await mcpStatus(good.access_token), wrongAudRefused: count(wrongAud, (s) => s === 401), foreignKeyRefused: count(foreignKey, (s) => s === 401),
    hostTokenRefused: count(hostTok, (s) => s === 401), expiredRefused: count(expired, (s) => s === 401), n: 100 };
  // The client's token never reaches /api/v3: graft-mcp's access token used directly at the host.
  const direct = await fetch(`${URLS.host}/api/v3/user`, { headers: { authorization: `Bearer ${good.access_token}` } });
  (res.aud as Record<string, unknown>).mcpTokenAtHost = direct.status;
  console.log("aud", JSON.stringify(res.aud));
}

// ---- refresh keeps scope; wider refused ----
{
  let t = await login("write:write");
  const before = await tools(t.access_token);
  let kept = 0, scopeKept = 0, widerRefused = 0;
  const widerBodies: string[] = [];
  for (let i = 0; i < 100; i++) {
    const wide = await token({ grant_type: "refresh_token", refresh_token: t.refresh_token, client_id: CIMD, scope: i % 2 ? "write:write_approve_merge" : "write:write tools:context,repos,issues,pull_requests,checks,coordination,provenance" });
    if (wide.status === 400 && wide.body.error === "invalid_scope") widerRefused++; else widerBodies.push(JSON.stringify(wide.body).slice(0, 150));
    const r = await token({ grant_type: "refresh_token", refresh_token: t.refresh_token, client_id: CIMD });
    if (r.status !== 200) { widerBodies.push(`refresh ${i}: ${JSON.stringify(r.body)}`); break; }
    t = r.body as typeof t;
    if (t.scope === "write:write tools:context,repos,issues,pull_requests") scopeKept++;
    if (i % 10 === 0 || i === 99) { if (JSON.stringify(await tools(t.access_token)) === JSON.stringify(before)) kept++; } else kept++;
  }
  // The host's own record of the newest scoped token: WriteScope from its Tokens row.
  res.refresh = { n: 100, scopeStringKept: scopeKept, toolsKept: kept, widerRefused, notes: widerBodies.slice(0, 3) };
  console.log("refresh", JSON.stringify(res.refresh));
  // ---- replay ----
  const t1 = await login("write:write");
  const r1 = await token({ grant_type: "refresh_token", refresh_token: t1.refresh_token, client_id: CIMD });
  const replay = await token({ grant_type: "refresh_token", refresh_token: t1.refresh_token, client_id: CIMD });
  const newest = await token({ grant_type: "refresh_token", refresh_token: (r1.body as typeof t1).refresh_token, client_id: CIMD });
  const at = await mcpStatus((r1.body as typeof t1).access_token);
  const call = await fetch(`${URLS.mcp}/`, { method: "POST", headers: { authorization: `Bearer ${(r1.body as typeof t1).access_token}`, "content-type": "application/json", accept: "application/json, text/event-stream",
    "mcp-protocol-version": "2026-07-28", "mcp-method": "tools/call", "mcp-name": "get_me" }, body: JSON.stringify({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "get_me", arguments: {}, _meta: {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": { name: "neg", version: "1" }, "io.modelcontextprotocol/clientCapabilities": {} } } }) });
  res.replay = { firstRefresh: r1.status, replay: replay.status, replayError: replay.body.error, replayDesc: replay.body.error_description,
    newestAfterReplay: newest.status, newestError: newest.body.error, accessTokenAfterReplayMcp: at, toolCallAfterReplay: (await call.text()).slice(0, 220) };
  console.log("replay", JSON.stringify(res.replay));
}

// ---- consent ----
{
  const a = await authorizeUrl("write:write");
  const page = await fetch(a.url);
  const html = await page.text();
  const p = /name=p value="([^"]+)"/.exec(html)![1].replace(/&#(\d+);/g, (_, c) => String.fromCharCode(Number(c)));
  const post = (h: Record<string, string>) => fetch(`${URLS.mcp}/consent`, { method: "POST", redirect: "manual", headers: { "content-type": "application/x-www-form-urlencoded", ...h },
    body: new URLSearchParams({ p, decision: "allow" }).toString() });
  const crossSite = await post({ origin: "https://evil.example", "sec-fetch-site": "cross-site" });
  const noOrigin = await post({});
  const framedSameOrigin = await post({ origin: URLS.mcp, "sec-fetch-site": "cross-site" });
  // Skipped consent: go straight to the host's OAuth and back to the callback, with no consent cookie.
  const host = new URL(`${URLS.host}/login/oauth/authorize`);
  host.searchParams.set("client_id", "Iv1.graftyardmcp"); host.searchParams.set("redirect_uri", `${URLS.mcp}/callback`); host.searchParams.set("state", crypto.randomUUID());
  const h = await fetch(host, { redirect: "manual", headers: { cookie: `user_session=${encodeURIComponent(await tok("bob"))}` } });
  const cbNoCookie = await fetch(h.headers.get("location")!, { redirect: "manual" });
  // Another flow's cookie (a real consent by mallory's attacker page) replayed with this callback's state.
  const ok = await post({ origin: URLS.mcp, "sec-fetch-site": "same-origin" });
  const otherCookie = (ok.headers.get("set-cookie") ?? "").split(";")[0];
  const h2 = await fetch(host, { redirect: "manual", headers: { cookie: `user_session=${encodeURIComponent(await tok("bob"))}` } });
  const cbOtherCookie = await fetch(h2.headers.get("location")!, { redirect: "manual", headers: { cookie: otherCookie } });
  // The legitimate path, then the same callback replayed (cookie once).
  const okLoc = ok.headers.get("location")!;
  const hostOk = await fetch(okLoc, { redirect: "manual", headers: { cookie: `user_session=${encodeURIComponent(await tok("bob"))}` } });
  const cb1 = await fetch(hostOk.headers.get("location")!, { redirect: "manual", headers: { cookie: otherCookie } });
  const cb2 = await fetch(hostOk.headers.get("location")!, { redirect: "manual", headers: { cookie: otherCookie } });
  const hasCode = (r: Response) => (r.headers.get("location") ?? "").includes("code=");
  // A code redeemed with the wrong verifier, and twice.
  const code = new URL(cb1.headers.get("location")!).searchParams.get("code")!;
  const badPkce = await token({ grant_type: "authorization_code", code, code_verifier: b64u(randomBytes(32)), client_id: CIMD, redirect_uri: REDIRECT });
  const good = await token({ grant_type: "authorization_code", code, code_verifier: a.verifier, client_id: CIMD, redirect_uri: REDIRECT });
  const again = await token({ grant_type: "authorization_code", code, code_verifier: a.verifier, client_id: CIMD, redirect_uri: REDIRECT });
  const noPkce = await fetch((await authorizeUrl()).url.replace(/code_challenge_method=S256/, "code_challenge_method=plain"), { redirect: "manual" });
  const wrongResource = await fetch((await authorizeUrl("write:write", CIMD, REDIRECT, `${URLS.host}/`)).url, { redirect: "manual" });
  res.consent = {
    pageHeaders: { csp: page.headers.get("content-security-policy"), xfo: page.headers.get("x-frame-options") },
    crossSitePost: crossSite.status, crossSiteCode: hasCode(crossSite), noOriginPost: noOrigin.status, framedSameOriginPost: framedSameOrigin.status,
    skippedConsentCallback: cbNoCookie.status, skippedConsentCode: hasCode(cbNoCookie), otherFlowCookie: cbOtherCookie.status, otherFlowCode: hasCode(cbOtherCookie),
    legitCallback: cb1.status, legitCode: hasCode(cb1), callbackReplay: cb2.status, callbackReplayCode: hasCode(cb2),
    badPkce: badPkce.status, goodPkce: good.status, codeReuse: again.status, plainPkce: noPkce.status, plainPkceLoc: (noPkce.headers.get("location") ?? "").replace(/.*error=/, "error="),
    wrongResource: (wrongResource.headers.get("location") ?? "").replace(/.*error=/, "error=").slice(0, 60) };
  console.log("consent", JSON.stringify(res.consent));
}

// ---- RFC 8693 exchange ----
{
  const t = await login("write:write");
  const ex = await token({ grant_type: "urn:ietf:params:oauth:grant-type:token-exchange", subject_token: t.access_token, subject_token_type: "urn:ietf:params:oauth:token-type:access_token", scope: "write:read" });
  const wider = await token({ grant_type: "urn:ietf:params:oauth:grant-type:token-exchange", subject_token: t.access_token, subject_token_type: "urn:ietf:params:oauth:token-type:access_token", scope: "write:write_approve_merge" });
  const otherRepo = await token({ grant_type: "urn:ietf:params:oauth:grant-type:token-exchange", subject_token: t.access_token, subject_token_type: "urn:ietf:params:oauth:token-type:access_token", scope: "write:read", repos: "acme/other" });
  const narrowed = ex.body.access_token as string;
  const read = await fetch(`${URLS.host}/api/v3/repos/acme/mono/issues/1`, { headers: { authorization: `Bearer ${narrowed}` } });
  const write = await fetch(`${URLS.host}/api/v3/repos/acme/mono/issues/1/comments`, { method: "POST", headers: { authorization: `Bearer ${narrowed}` }, body: JSON.stringify({ body: "x" }) });
  const other = await fetch(`${URLS.host}/api/v3/repos/acme/other/issues/1`, { headers: { authorization: `Bearer ${narrowed}` } });
  res.exchange = { status: ex.status, issued: ex.body.issued_token_type, scope: ex.body.scope, wider: wider.status, widerError: wider.body.error, otherRepo: otherRepo.status,
    narrowedRead: read.status, narrowedWrite: write.status, narrowedWriteMsg: (await write.text()).slice(0, 120), narrowedOtherRepo: other.status };
  console.log("exchange", JSON.stringify(res.exchange));
}

// ---- CIMD hardening ----
{
  const out: Record<string, string> = {};
  for (const n of ["big", "redirect", "wrong-id", "slow"]) {
    const a = await authorizeUrl("write:read", `${URLS.aux}/cimd/${n}.json`);
    const t0 = Date.now(); const r = await fetch(a.url, { redirect: "manual" });
    out[n] = `${r.status} ${(await r.text()).slice(0, 70)} (${Date.now() - t0} ms)`;
  }
  for (const [n, c] of [["http", CIMD.replace("https:", "http:")], ["denied-zone", `${URLS.host}/cimd/x.json`], ["root-path", `${URLS.aux}/`]]) {
    const r = await fetch((await authorizeUrl("write:read", c)).url, { redirect: "manual" });
    out[n] = `${r.status} ${(await r.text()).slice(0, 70)}`;
  }
  const unreg = await fetch((await authorizeUrl("write:read", CIMD, "http://127.0.0.1:9999/evil")).url, { redirect: "manual" });
  out["unregistered-redirect"] = `${unreg.status} ${(await unreg.text()).slice(0, 70)}`;
  res.cimd = out;
  console.log("cimd", JSON.stringify(out));
}

// ---- ghr_ after DELETE of the unscoped ghu_ (prototype only) ----
{
  const a = await authorizeUrl("write:read");
  const { callback } = await consentAndCallback(a.url);
  const t = await token({ grant_type: "authorization_code", code: new URL(callback).searchParams.get("code")!, code_verifier: a.verifier, client_id: CIMD, redirect_uri: REDIRECT });
  const r = await token({ grant_type: "refresh_token", refresh_token: t.body.refresh_token, client_id: CIMD });
  res.ghr = { firstRefreshAfterDelete: r.status, note: "graft-mcp deleted the unscoped ghu_ at the callback; the prototype host keeps the ghr_ row. Real GHES behavior untested (needs GHES)." };
  console.log("ghr", JSON.stringify(res.ghr));
}

writeFileSync(`${ROOT}results/raw/wp15-l008-neg.json`, JSON.stringify(res, null, 2));
