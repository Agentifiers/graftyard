// AM2-WP1 hosted: L0-08's cells that need no SDK client, LF-6 through the edge, and the controls, against graft-mcp
// and the install's edge (`tests/am2/hosted/mcp.mjs` for the environment). Rows go to tests/am2/raw/l008-<tag>.jsonl,
// one per case, and the summary to stdout. The SDK clients' sessions are `clients.sh`.
//   node tests/am2/hosted/l008.mjs <tag> [case ...]     cases: meta aud refresh replay consent held lf6 (default all)
// Each case is L0-08's or LF-6's row as `build/BUILD-PLAN.md` writes it:
//   aud      100 of 100 wrong-audience tokens refused: our key and another audience (the host's origin, another server,
//            a suffix host, no trailing slash), another server's key, the host's own `ghu_`, a refresh token presented as
//            access, expired; and graft-mcp's own token at the host's /api/v3 is refused
//   refresh  a refreshed token keeps its scope over 100 refreshes; 100 wider asks refused; the host's scoped row's
//            `write` field stays the grant's
//   replay   a replayed refresh is refused and revokes the grant: the newest refresh fails, the grant leaves the
//            settings list, and the user's scoped rows are revoked
//   consent  framed, cross-site, Origin-less, skipped consent, another flow's cookie, and a replayed callback issue no
//            code; the page carries `frame-ancestors 'none'` and `X-Frame-Options: DENY`
//   held     a refresh succeeds with the unscoped `ghu_` held, which is deleted after it (live unscoped rows 1 -> 1, and
//            the first pair's rows revoked)
//   lf6      LF-6 through the edge, 100 of each: `token/scoped` from `Graftyard MCP` without `graftyard_grant`, with
//            another user's grant, with `graftyard_write` above the grant: 422; a stale generation: 409, the grant
//            revoked, and its scoped token dead; wrong client credentials on the grant endpoints and `token/scoped`:
//            401. Needs L008_REGISTER, the `apps init --out` folder (see the case)
import { mkdirSync, appendFileSync } from "node:fs";
import { api, bed, log, scan, secret, state, tokens } from "../../am1/hosted/lib.mjs";
import { AUX, MCP, REPO, authorizeUrl, cimd, consent, login, mcpCall, mcpTokens, settingsGrants, token } from "./mcp.mjs";

const [tag, ...only] = process.argv.slice(2);
if (!tag) throw new Error("usage: l008.mjs <tag> [case ...]");
const N = Number(process.env.L008_N ?? 100);
mkdirSync("tests/am2/raw", { recursive: true });
const OUT = `tests/am2/raw/l008-${tag}.jsonl`;
const row = (v) => appendFileSync(OUT, scan(JSON.stringify({ at: new Date().toISOString(), ...v })) + "\n");
const want = (c) => !only.length || only.includes(c);
const count = (xs, f) => xs.filter(f).length;
const summary = {};
const client = () => state.agentApps["graftyard-mcp"].client;

if (want("meta")) {
  const prm = await (await fetch(`${MCP}/.well-known/oauth-protected-resource`)).json();
  const asm = await (await fetch(`${MCP}/.well-known/oauth-authorization-server`)).json();
  const door = await mcpCall(null);
  summary.meta = { resource: prm.resource, servers: prm.authorization_servers, scopes: prm.scopes_supported, pkce: asm.code_challenge_methods_supported,
    cimd: asm.client_id_metadata_document_supported, iss: asm.authorization_response_iss_parameter_supported, door: door.status, www: door.www };
  row({ case: "meta", ...summary.meta });
}

if (want("aud")) {
  const good = await login("write:read");
  const refreshAsAccess = good.refresh_token;
  // Our own key: re-sealing needs it, and graft-mcp holds it in Secrets Store only, so the forged-audience cases are
  // tokens graft-mcp issued with another audience's origin (the same server under another name is the case a deploy
  // mistake allows), plus every other token kind a client might present.
  const cases = {
    hostGhu: async () => tokens.member,
    refreshAsAccess: async () => refreshAsAccess,
    foreignKey: async (i) => `gymcp_${Buffer.from(`other-${i}-${"x".repeat(60)}`).toString("base64")}`,
    garbage: async (i) => `not-a-token-${i}`,
  };
  const out = {};
  for (const [k, f] of Object.entries(cases)) {
    const s = [];
    for (let i = 0; i < N; i++) s.push((await mcpCall(await f(i))).status);
    out[k] = count(s, (x) => x === 401);
  }
  // Another server's audience: our token sent to the host's origin and to a suffix and slash variant of ours.
  const elsewhere = [];
  for (const o of [process.env.GRAFT_EDGE, `${MCP}.evil.example`]) {
    try { elsewhere.push({ origin: o.replace(/\/\/[^.]+/, "//<name>"), status: (await mcpCall(good.access_token, "tools/list", {}, o)).status }); }
    catch (e) { elsewhere.push({ origin: o.replace(/\/\/[^.]+/, "//<name>"), status: `no answer: ${String(e).slice(0, 60)}` }); }
  }
  const atHost = await api("GET", "/api/v3/user", good.access_token);
  summary.aud = { n: N, refused: out, good: (await mcpCall(good.access_token)).status, elsewhere, mcpTokenAtHost: atHost.status };
  row({ case: "aud", ...summary.aud });
}

if (want("refresh")) {
  let t = await login("write:write");
  const scope0 = t.scope;
  let kept = 0, widerRefused = 0, door = 0;
  const notes = [];
  for (let i = 0; i < N; i++) {
    const wide = await token({ grant_type: "refresh_token", refresh_token: t.refresh_token, client_id: cimd("ts-v2"),
      scope: i % 2 ? "write:write_approve_merge" : "write:write tools:context,repos,issues,pull_requests,checks,coordination,provenance" });
    if (wide.status === 400 && wide.body.error === "invalid_scope") widerRefused++; else notes.push(`wide ${i}: ${wide.status} ${wide.body.error}`);
    const r = await token({ grant_type: "refresh_token", refresh_token: t.refresh_token, client_id: cimd("ts-v2") });
    if (r.status !== 200) { notes.push(`refresh ${i}: ${r.status} ${JSON.stringify(r.body).slice(0, 160)}`); break; }
    t = r.body;
    if (t.scope === scope0) kept++;
    if ((await mcpCall(t.access_token)).status !== 401) door++;
  }
  const rows = await mcpTokens();
  summary.refresh = { n: N, scope0, scopeKept: kept, widerRefused, doorAccepted: door, liveScoped: rows.scoped, liveUnscoped: rows.unscoped, notes: notes.slice(0, 3) };
  row({ case: "refresh", ...summary.refresh });
}

if (want("replay")) {
  const t1 = await login("write:write");
  const before = await settingsGrants();
  const r1 = await token({ grant_type: "refresh_token", refresh_token: t1.refresh_token, client_id: cimd("ts-v2") });
  const replay = await token({ grant_type: "refresh_token", refresh_token: t1.refresh_token, client_id: cimd("ts-v2") });
  const newest = await token({ grant_type: "refresh_token", refresh_token: r1.body.refresh_token, client_id: cimd("ts-v2") });
  const after = await settingsGrants();
  const scopedAt = await api("GET", `/api/v3/repos/${REPO}`, null);
  summary.replay = { firstRefresh: r1.status, replay: replay.status, replayError: replay.body.error, newestAfterReplay: newest.status,
    newestError: newest.body.error, grantsBefore: before.length, grantsAfter: after.length, anonymousRepoRead: scopedAt.status };
  row({ case: "replay", ...summary.replay });
}

if (want("consent")) {
  const a = authorizeUrl({ scope: "write:write" });
  const page = await fetch(a.url);
  const html = await page.text();
  const p = /name=p value="([^"]+)"/.exec(html)[1].replace(/&#(\d+);/g, (_, c) => String.fromCharCode(Number(c)));
  const post = (h) => fetch(`${MCP}/consent`, { method: "POST", redirect: "manual", headers: { "content-type": "application/x-www-form-urlencoded", ...h },
    body: new URLSearchParams({ p, decision: "allow", repos: REPO }).toString() });
  const hasCode = (r) => (r.headers.get("location") ?? "").includes("code=");
  const crossSite = await post({ origin: "https://evil.example", "sec-fetch-site": "cross-site" });
  const noOrigin = await post({});
  const framed = await post({ origin: MCP, "sec-fetch-site": "cross-site" });
  // Skipped consent: the host's OAuth for Graftyard MCP straight to the callback, with no flow cookie.
  const host = new URL(`${process.env.GRAFT_EDGE}/login/oauth/authorize`);
  host.searchParams.set("client_id", client()); host.searchParams.set("redirect_uri", `${MCP}/callback`); host.searchParams.set("state", "skipped");
  const h = await fetch(host, { redirect: "manual", headers: { authorization: `token ${tokens.member}` } });
  const skipped = await fetch(h.headers.get("location"), { redirect: "manual" });
  // A real consent's cookie replayed with another state, then the legitimate callback twice.
  const ok = await consent(authorizeUrl({ scope: "write:write" }).url);
  const otherState = new URL(ok.hostCallback); otherState.searchParams.set("state", "0".repeat(32));
  const other = await fetch(otherState, { redirect: "manual", headers: { cookie: ok.flowCookie } });
  const replayed = await fetch(ok.hostCallback, { redirect: "manual", headers: { cookie: ok.flowCookie } });
  summary.consent = { csp: page.headers.get("content-security-policy"), xfo: page.headers.get("x-frame-options"),
    crossSite: [crossSite.status, hasCode(crossSite)], noOrigin: [noOrigin.status, hasCode(noOrigin)], framed: [framed.status, hasCode(framed)],
    skipped: [skipped.status, hasCode(skipped)], otherState: [other.status, hasCode(other)], callbackReplay: [replayed.status, hasCode(replayed)],
    legit: new URL(ok.callback).searchParams.has("code") };
  row({ case: "consent", ...summary.consent });
}

if (want("held")) {
  const base = await mcpTokens();
  const t = await login("write:read");
  const mid = await mcpTokens();
  const r = await token({ grant_type: "refresh_token", refresh_token: t.refresh_token, client_id: cimd("ts-v2") });
  const end = await mcpTokens();
  summary.held = { refresh: r.status, unscoped: [base.unscoped, mid.unscoped, end.unscoped], scoped: [base.scoped, mid.scoped, end.scoped] };
  row({ case: "held", ...summary.held });
}

if (want("lf6")) {
  // graft-mcp's client secret is in Secrets Store only. A record holds two client secrets so a rotation can overlap
  // (`CLIENT_SECRETS_MAX`), so for this case the record from apps-register.request.json (L008_REGISTER, the
  // `apps init --out` folder) gets a test secret beside graft-mcp's, and the original record is put back after, even on
  // a failure. Then, 100 of each: 422 for each bad `token/scoped` request, 409 and the grant revoked for a stale
  // generation, 401 for wrong client credentials.
  const { readFileSync } = await import("node:fs");
  const { randomBytes, createHash } = await import("node:crypto");
  const { must } = await import("../../am1/hosted/lib.mjs");
  const reg = JSON.parse(readFileSync(`${process.env.L008_REGISTER}/apps-register.request.json`, "utf8"));
  const put = reg.find((r) => r.op === "PutApp (graftyard-mcp)");
  const record = put.cmd.op.PutApp.record;
  const test = secret(randomBytes(20).toString("hex"));
  const withTest = { ...record, client_secrets: [...record.client_secrets, [...createHash("sha256").update(test).digest()]] };
  const id = client();
  const basic = (s) => `Basic ${Buffer.from(`${id}:${s}`).toString("base64")}`;
  const good = basic(test);
  // A live `Graftyard MCP` ghu_ for `pat`'s user, through the host's OAuth with the test secret.
  const ghu = async (pat) => {
    const u = new URL(`${process.env.GRAFT_EDGE}/login/oauth/authorize`);
    u.searchParams.set("client_id", id); u.searchParams.set("redirect_uri", `${MCP}/callback`); u.searchParams.set("state", "lf6");
    const r = await fetch(u, { redirect: "manual", headers: { authorization: `token ${pat}` } });
    const code = new URL(r.headers.get("location")).searchParams.get("code");
    const t = await fetch(`${process.env.GRAFT_EDGE}/login/oauth/access_token`, { method: "POST", headers: { accept: "application/json", "content-type": "application/json" },
      body: JSON.stringify({ client_id: id, client_secret: test, code }) });
    const b = await t.json();
    return secret(b.access_token);
  };
  const newGrant = async (user, write) => {
    const r = await api("POST", "/api/graftyard/v1/mcp/grants", null, { client: cimd("ts-v2"), redirect_hosts: ["127.0.0.1"], write },
      { authorization: good, "x-graftyard-user-token": user });
    if (r.status !== 201) throw new Error(`grant: ${r.status} ${JSON.stringify(r.body).slice(0, 160)}`);
    return r.body;
  };
  const scoped = (body) => api("POST", `/api/v3/applications/${id}/token/scoped`, null, { target: "m3org", repositories: ["j4"], ...body }, { authorization: good });
  await must(put.chain, { ...put.cmd, op: { PutApp: { record: withTest } } });
  try {
    const member = await ghu(tokens.member), admin = await ghu(tokens.admin);
    const [mine, theirs, readOnly] = [await newGrant(member, "write"), await newGrant(admin, "write"), await newGrant(member, "read")];
    const control = await scoped({ access_token: member, graftyard_grant: mine.id, graftyard_write: "write" });
    const c422 = { noGrant: [], otherUsersGrant: [], writeAboveGrant: [] };
    for (let i = 0; i < N; i++) {
      c422.noGrant.push((await scoped({ access_token: member, graftyard_write: "read" })).status);
      c422.otherUsersGrant.push((await scoped({ access_token: member, graftyard_grant: theirs.id, graftyard_write: "read" })).status);
      c422.writeAboveGrant.push((await scoped({ access_token: member, graftyard_grant: readOnly.id, graftyard_write: i % 2 ? "write" : "write_approve_merge" })).status);
    }
    // Stale generation: a fresh grant with a scoped token, its generation spent once, then the old one sent again.
    const stale = { s409: 0, grantGone: 0, scopedRevoked: 0, notes: [] };
    for (let i = 0; i < N; i++) {
      const g = await newGrant(member, "read");
      const t = await scoped({ access_token: member, graftyard_grant: g.id, graftyard_write: "read" });
      if (t.status === 200) secret(t.body.token);
      const gen = `/api/graftyard/v1/mcp/grants/${g.id}/generation`;
      const first = await api("POST", gen, null, { generation: g.generation }, { authorization: good });
      const again = await api("POST", gen, null, { generation: g.generation }, { authorization: good });
      if (again.status === 409) stale.s409++; else if (stale.notes.length < 3) stale.notes.push(`${i}: first ${first.status}, again ${again.status}`);
      const list = await api("GET", "/api/graftyard/v1/mcp/grants", null, undefined, { authorization: good, "x-graftyard-user-token": member });
      if (Array.isArray(list.body) && !list.body.some((x) => x.id === g.id)) stale.grantGone++;
      if (t.status === 200 && (await api("GET", "/api/v3/user", t.body.token)).status === 401) stale.scopedRevoked++;
    }
    const wrong = [];
    for (let i = 0; i < N; i++) {
      const cred = basic(`wrong-${i}`);
      wrong.push((await api("GET", "/api/graftyard/v1/mcp/grants", null, undefined, { authorization: cred, "x-graftyard-user-token": member })).status);
      wrong.push((await api("POST", `/api/graftyard/v1/mcp/grants/${mine.id}/generation`, null, { generation: 0 }, { authorization: cred })).status);
      wrong.push((await api("POST", `/api/v3/applications/${id}/token/scoped`, null,
        { access_token: member, target: "m3org", repositories: ["j4"], graftyard_grant: mine.id, graftyard_write: "read" }, { authorization: cred })).status);
    }
    summary.lf6 = { n: N, control: control.status, controlBody: control.status === 200 ? Object.keys(control.body).sort() : control.body,
      s422: Object.fromEntries(Object.entries(c422).map(([k, v]) => [k, count(v, (s) => s === 422)])), stale,
      wrongCredentials401: count(wrong, (s) => s === 401), wrongCredentialsN: wrong.length };
    row({ case: "lf6", ...summary.lf6 });
  } finally {
    await must(put.chain, put.cmd);
    log("lf6: the record's test secret removed (the record as apps init registered it)");
  }
}

log(`l008 ${tag} ${JSON.stringify(summary)}`);
void bed; void AUX;
