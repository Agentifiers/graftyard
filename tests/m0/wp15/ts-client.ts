// M0-WP15, L0-08: the official TypeScript SDK clients against graft-mcp, unmodified. Each run is one client:
//   node wp15/ts-client.ts v2 [calls]     @modelcontextprotocol/client 2.4.0, versionNegotiation pin 2026-07-28
//   node wp15/ts-client.ts v1 [calls]     @modelcontextprotocol/sdk 1.32.2, which speaks 2025-11-25 (initialize)
// The SDK's own auth() does discovery (RFC 9728 from WWW-Authenticate, RFC 8414), CIMD (`clientMetadataUrl`), PKCE,
// RFC 8707 `resource`, and the RFC 9207 iss check; this file supplies only the OAuthClientProvider storage and the
// "open a browser" step (run/wp15-browser.ts). Prints one JSON line of results.

import { consentAndCallback } from "../run/wp15-browser.ts";
import { URLS, tok } from "../run/wp15.ts";

const which = process.argv[2] === "v1" ? "v1" : "v2";
const CALLS = Number(process.argv[3] ?? 30);
const name = which === "v1" ? "ts-v1" : "ts-v2";
const port = 4710 + ["ts-v2", "py-v2", "go-v18", "ts-v1"].indexOf(name);
const cimd = `${URLS.aux}/cimd/${name}.json`;
const redirect = `http://127.0.0.1:${port}/callback`;

// One provider shape serves both SDK majors.
let tokens: any, verifier = "", pendingAuth: URL | null = null, discovery: any;
const provider: any = {
  get redirectUrl() { return redirect; },
  clientMetadataUrl: cimd,
  get clientMetadata() { return { client_name: name, redirect_uris: [redirect], grant_types: ["authorization_code", "refresh_token"], response_types: ["code"], token_endpoint_auth_method: "none",
    scope: process.env.WP15_SCOPE ?? "write:write" }; },
  clientInformation: () => ({ client_id: cimd }),
  saveClientInformation: () => undefined,
  tokens: () => tokens,
  saveTokens: (t: any) => { tokens = t; },
  redirectToAuthorization: (u: URL) => { pendingAuth = u; },
  saveCodeVerifier: (v: string) => { verifier = v; },
  codeVerifier: () => verifier,
  saveDiscoveryState: (s: any) => { discovery = s; },
  discoveryState: () => discovery,
  state: () => crypto.randomUUID(),
};

async function sdk() {
  if (which === "v2") {
    const m = await import("@modelcontextprotocol/client");
    return { Client: m.Client, Transport: m.StreamableHTTPClientTransport, Unauthorized: m.UnauthorizedError,
      opts: { versionNegotiation: { mode: { pin: "2026-07-28" } } } };
  }
  const c = await import("sdkv1/client/index.js"), t = await import("sdkv1/client/streamableHttp.js"), a = await import("sdkv1/client/auth.js");
  return { Client: c.Client, Transport: t.StreamableHTTPClientTransport, Unauthorized: a.UnauthorizedError, opts: {} };
}

const { Client, Transport, Unauthorized, opts } = await sdk() as any;
const out: Record<string, unknown> = { client: name, sdk: which === "v2" ? "@modelcontextprotocol/client 2.4.0" : "@modelcontextprotocol/sdk 1.32.2" };
const url = new URL(`${URLS.mcp}/`);
// Server-Timing from every MCP POST: graft-mcp's own time and the REST time inside it (overhead = mcp - edge).
const timings: { mcp: number; edge: number }[] = [];
const meteredFetch = async (u: any, init?: any) => { const r = await fetch(u, init);
  const st = r.headers.get("server-timing"); const m = st && /mcp;dur=(\d+), edge;dur=(\d+)/.exec(st);
  if (m && /"tools\/call"/.test(String(init?.body ?? ""))) timings.push({ mcp: Number(m[1]), edge: Number(m[2]) }); return r; };
let transport = new Transport(url, { authProvider: provider, fetch: meteredFetch });
let client = new Client({ name, version: "1.0.0" }, opts);
const t0 = Date.now();
try { await client.connect(transport); }
catch (e) {
  if (!(e instanceof Unauthorized) || !pendingAuth) throw e;
  out.authUrl = String(pendingAuth);
  const { callback, seen } = await consentAndCallback(String(pendingAuth));
  out.consentHeaders = { csp: seen.csp, xfo: seen.xfo };
  const cb = new URL(callback);
  out.iss = cb.searchParams.get("iss");
  // The SDK checks RFC 9207 `iss` when it is passed; v1 has no iss parameter on finishAuth.
  if (which === "v2") await transport.finishAuth(cb.searchParams); else await transport.finishAuth(cb.searchParams.get("code"));
  transport = new Transport(url, { authProvider: provider, fetch: meteredFetch });
  client = new Client({ name, version: "1.0.0" }, opts);
  await client.connect(transport);
}
out.authMs = Date.now() - t0;
out.protocol = which === "v2" ? client.getNegotiatedProtocolVersion?.() : (transport as any).protocolVersion ?? "2025-11-25";
const tools = await client.listTools();
out.tools = tools.tools.map((t: { name: string }) => t.name);
let okN = 0;
const lat: number[] = [];
const errors: string[] = [];
const direct: number[] = [];
const bobTok = await tok("bob");
for (let i = 0; i < CALLS; i++) {
  const s = Date.now();
  const r = i % 3 === 0 ? await client.callTool({ name: "get_me", arguments: {} })
    : i % 3 === 1 ? await client.callTool({ name: "get_ref", arguments: { owner: "acme", repo: "mono", ref: "heads/main" } })
    : await client.callTool({ name: "get_issue", arguments: { owner: "acme", repo: "mono", number: 1 } });
  lat.push(Date.now() - s);
  if (!r.isError) okN++; else errors.push(JSON.stringify(r.content).slice(0, 200));
  // The same REST request straight to the host with bob's harness token, for the client-side overhead.
  const path = i % 3 === 0 ? "/api/v3/user" : i % 3 === 1 ? "/api/v3/repos/acme/mono/git/ref/heads/main" : "/api/v3/repos/acme/mono/issues/1";
  const d = Date.now(); await (await fetch(`${URLS.host}${path}`, { headers: { authorization: `Bearer ${bobTok}`, accept: "application/vnd.graftyard.agent+json" } })).text();
  direct.push(Date.now() - d);
}
direct.sort((a, b) => a - b);
out.directP50 = direct[Math.floor(direct.length / 2)];
lat.sort((a, b) => a - b);
const ov = timings.map((t) => t.mcp - t.edge).sort((a, b) => a - b);
Object.assign(out, { overheadP50: ov[Math.floor(ov.length / 2)] ?? null, overheadN: ov.length, edgeP50: timings.map((t) => t.edge).sort((a, b) => a - b)[Math.floor(timings.length / 2)] ?? null, calls: CALLS, ok: okN, clientP50: lat[Math.floor(lat.length / 2)], clientOverheadP50: lat[Math.floor(lat.length / 2)] - (out.directP50 as number), errors: errors.slice(0, 3), tokenScope: tokens?.scope, refreshToken: !!tokens?.refresh_token });
await client.close();
console.log(JSON.stringify(out));
