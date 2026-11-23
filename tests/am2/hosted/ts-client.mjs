// L0-08 hosted and A43 with the official TypeScript SDK clients, unmodified, against graft-mcp:
//   node tests/am2/hosted/ts-client.mjs v2|v1 [calls] [--drop-token]
// Each call is `list_issues` on m3org/j4, interleaved with the same REST read sent straight to the edge with the member's
// PAT; the tool overhead is the p50 through graft-mcp minus the direct p50 (M0-WP15's method, `tests/m0/results/m0-wp15.md`).
// `@modelcontextprotocol/client` 2.4.0 (pin 2026-07-28) or `@modelcontextprotocol/sdk` 1.32.2 (2025-11-25), from
// tests/m0/wp15/node_modules in the main checkout. The SDK's own auth() does RFC 9728 discovery, RFC 8414, CIMD, PKCE S256,
// RFC 8707 `resource`, and the RFC 9207 iss check; this file supplies only token storage and the browser step.
// `--drop-token` (A43): the first `/token` answer that redeems a code is dropped after graft-mcp answered it, as a lost
// response would be, and the client is left to do what it does. The row records whether it redeemed again, what that
// answered, whether the grant survived (the settings list), and whether the session needed a second consent.
// Prints one JSON line.
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { EDGE } from "../../am1/hosted/lib.mjs";
import { MCP, MEMBER, cimd, consent, redirectOf, settingsGrants } from "./mcp.mjs";

// M0-WP15's harness modules, in the main checkout (GRAFT_MAIN_CHECKOUT, as lib.mjs reads it).
const main = process.env.GRAFT_MAIN_CHECKOUT ?? `${homedir()}/workspace/repo`;
const req = createRequire(`${process.env.WP15_MODULES ?? `${main}/tests/m0/wp15`}/package.json`);
const which = process.argv[2] === "v1" ? "v1" : "v2";
const CALLS = Number(process.argv[3] ?? 30);
const DROP = process.argv.includes("--drop-token");
const name = which === "v1" ? "ts-v1" : "ts-v2";
const redirect = redirectOf(name);

let tokens, verifier = "", pendingAuth = null, discovery;
const provider = {
  get redirectUrl() { return redirect; },
  clientMetadataUrl: cimd(name),
  get clientMetadata() { return { client_name: name, redirect_uris: [redirect], grant_types: ["authorization_code", "refresh_token"], response_types: ["code"],
    token_endpoint_auth_method: "none", scope: process.env.MCP_SCOPE ?? "write:write" }; },
  clientInformation: () => ({ client_id: cimd(name) }),
  saveClientInformation: () => undefined,
  tokens: () => tokens,
  saveTokens: (t) => { tokens = t; },
  redirectToAuthorization: (u) => { pendingAuth = u; },
  saveCodeVerifier: (v) => { verifier = v; },
  codeVerifier: () => verifier,
  saveDiscoveryState: (s) => { discovery = s; },
  discoveryState: () => discovery,
  state: () => crypto.randomUUID(),
};

async function sdk() {
  if (which === "v2") {
    const m = await import(req.resolve("@modelcontextprotocol/client"));
    return { Client: m.Client, Transport: m.StreamableHTTPClientTransport, Unauthorized: m.UnauthorizedError, opts: { versionNegotiation: { mode: { pin: "2026-07-28" } } } };
  }
  const base = req.resolve("sdkv1/client/index.js").replace(/index\.js$/, "");
  const c = await import(`${base}index.js`), t = await import(`${base}streamableHttp.js`), a = await import(`${base}auth.js`);
  return { Client: c.Client, Transport: t.StreamableHTTPClientTransport, Unauthorized: a.UnauthorizedError, opts: {} };
}

const { Client, Transport, Unauthorized, opts } = await sdk();
const out = { client: name, sdk: which === "v2" ? "@modelcontextprotocol/client 2.4.0" : "@modelcontextprotocol/sdk 1.32.2", drop: DROP };
// Every /token call the SDK makes, in order: the grant type and graft-mcp's status; the first code redemption's
// answer is dropped under --drop-token.
const tokenCalls = [];
let dropped = false;
const fetchFn = async (u, init) => {
  const url = String(u instanceof Request ? u.url : u);
  const r = await fetch(u, init);
  if (url === `${MCP}/token`) {
    const grant = new URLSearchParams(String(init?.body ?? "")).get("grant_type");
    tokenCalls.push({ grant, status: r.status });
    if (DROP && !dropped && grant === "authorization_code") {
      dropped = true;
      await r.arrayBuffer();
      throw new TypeError("fetch failed (A43: the /token answer dropped after graft-mcp answered)");
    }
  }
  return r;
};
const url = new URL(`${MCP}/`);
let consents = 0;
async function connect() {
  for (let attempt = 0; attempt < 3; attempt++) {
    const transport = new Transport(url, { authProvider: provider, fetch: fetchFn });
    const client = new Client({ name, version: "1.0.0" }, opts);
    try {
      await client.connect(transport);
      return { client, transport };
    } catch (e) {
      if (!(e instanceof Unauthorized) || !pendingAuth) throw e;
      consents++;
      const { callback, seen } = await consent(String(pendingAuth));
      pendingAuth = null;
      out.consentHeaders = { csp: seen.csp, xfo: seen.xfo };
      const cb = new URL(callback);
      out.iss = cb.searchParams.get("iss");
      try {
        if (which === "v2") await transport.finishAuth(cb.searchParams); else await transport.finishAuth(cb.searchParams.get("code"));
      } catch (f) {
        out.finishAuthError = String(f).slice(0, 200);
        // A client that does not retry the code itself: its next connect starts again from the 401.
      }
    }
  }
  throw new Error("no session after three connects");
}

const t0 = Date.now();
try {
  const { client, transport } = await connect();
  out.authMs = Date.now() - t0;
  out.protocol = which === "v2" ? client.getNegotiatedProtocolVersion?.() : transport.protocolVersion ?? "2025-11-25";
  let ok = 0;
  const lat = [], direct = [], errors = [];
  const p50 = (a) => a.sort((x, y) => x - y)[Math.floor(a.length / 2)];
  for (let i = 0; i < CALLS; i++) {
    const s = Date.now();
    try {
      const r = await client.callTool({ name: "list_issues", arguments: { owner: "m3org", repo: "j4", per_page: 5 } });
      if (!r.isError) ok++; else errors.push(JSON.stringify(r.content).slice(0, 160));
    } catch (e) { errors.push(String(e).slice(0, 160)); }
    lat.push(Date.now() - s);
    const d = Date.now();
    await (await fetch(`${EDGE}/api/v3/repos/m3org/j4/issues?per_page=5`, { headers: { authorization: `token ${MEMBER}`,
      accept: "application/vnd.graftyard.agent+json", "user-agent": "am2-direct" } })).text();
    direct.push(Date.now() - d);
  }
  const clientP50 = p50(lat), directP50 = p50(direct);
  Object.assign(out, { calls: CALLS, ok, clientP50, directP50, overheadP50: clientP50 - directP50, errors: errors.slice(0, 3) });
  await client.close();
} catch (e) {
  out.error = String(e).slice(0, 300);
}
Object.assign(out, { consents, tokenCalls, codeRedemptions: tokenCalls.filter((c) => c.grant === "authorization_code").length,
  grantsListed: await settingsGrants().then((gs) => gs.filter((g) => g.client === cimd(name)).length, (e) => `unread: ${String(e).slice(0, 80)}`), tokenScope: tokens?.scope, refreshToken: !!tokens?.refresh_token });
console.log(JSON.stringify(out));
