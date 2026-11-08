// BF-36 on graft-m2b (`build/BUILD-PLAN.md`, M3-WP3), against the test IdP (tests/m3/rows/oidc-idp.mjs, an orb
// service reached by the edge through a quick tunnel whose URL is <state>/oidc/issuer), `runs` times:
// - m3-member's `PUT /api/graftyard/v1/admin/instance-auth` to OIDC is refused 403 and changes nothing;
// - m3-admin, an admin of the install org (`SetMember` on `Authz(1)` as the operator, as tests/m3/hosted/rows.ts
//   does), switches it to OIDC (204); `GET` then reads `oidc` with the issuer and client, never the secret;
// - the next sign-in: `GET /login/oidc` (302 to the IdP's `/authorize`, with the state cookie), the IdP's redirect
//   with `login_hint=m3-admin`, and the callback with the cookie: 201, method `Oidc`, m3-admin's account. A
//   `SignIn { method: Password }` committed on m3-admin's `Identity` chain is then `WrongMethod`;
// - m3-admin switches back to Builtin (204), after which `GET /login/oidc` is 404 and a Password sign-in commits.
// The client secret is read from <state>/oidc/secret and sent only in the PUT's body. Rows hold statuses only.
//   node tests/m3/rows/bf36.mjs <runs> <out.jsonl>
import { readFileSync } from "node:fs";
import { EDGE, INSTALL, OP, STATE, commit, log, must, refusal, rid, row, secret, state, tokens } from "./lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 10);
const ISSUER = readFileSync(`${STATE}/oidc/issuer`, "utf8").trim();
const SECRET = secret(readFileSync(`${STATE}/oidc/secret`, "utf8").trim());
const CLIENT = "graftyard-m2b";
const install = `authz:${INSTALL}`;
await must(install, { cred: OP, op: { SetMember: { user: state.admin, role: "Admin" } } });

async function setAuth(token, body) {
  const r = await fetch(`${EDGE}/api/graftyard/v1/admin/instance-auth`, {
    method: "PUT", signal: AbortSignal.timeout(120_000),
    headers: { authorization: `token ${token}`, "content-type": "application/json", accept: "application/json" }, body: JSON.stringify(body),
  });
  await r.text();
  return r.status;
}
async function getAuth(token) {
  const r = await fetch(`${EDGE}/api/graftyard/v1/admin/instance-auth`, { headers: { authorization: `token ${token}` }, signal: AbortSignal.timeout(60_000) });
  const t = await r.text();
  if (t.includes(SECRET)) throw new Error("GET instance-auth returned the client secret");
  return JSON.parse(t);
}
/** The browser's OIDC sign-in, as m3-admin at the IdP: start, the IdP's redirect, the callback with the cookie. */
async function oidcSignIn(hint) {
  const s = await fetch(`${EDGE}/login/oidc`, { redirect: "manual", signal: AbortSignal.timeout(60_000) });
  const start = s.status;
  if (start !== 302) { await s.text(); return { start }; }
  const cookie = (s.headers.get("set-cookie") ?? "").split(";")[0];
  const to = new URL(s.headers.get("location"));
  const toIdp = to.origin === ISSUER;
  to.searchParams.set("login_hint", hint);
  const a = await fetch(to, { redirect: "manual", signal: AbortSignal.timeout(60_000) });
  await a.text();
  const back = a.headers.get("location");
  if (a.status !== 302 || !back) return { start, toIdp, authorize: a.status };
  // Without the cookie the callback is refused before the code is spent (the state is this browser's).
  const bare = await fetch(back, { redirect: "manual", signal: AbortSignal.timeout(120_000) });
  await bare.text();
  const cb = await fetch(back, { redirect: "manual", headers: { cookie }, signal: AbortSignal.timeout(120_000) });
  const b = await cb.json().catch(() => null);
  return { start, toIdp, authorize: a.status, noCookie: bare.status, callback: cb.status, method: b?.method ?? b?.message?.slice(0, 80), account: b?.account ?? null };
}
const password = async () => refusal(await commit(`identity:${state.admin}`, { cred: OP, op: { SignIn: { id: rid(), method: "Password", install: INSTALL } } }));

const c = { runs, ok: 0, fail: [] };
for (let i = 1; i <= runs; i++) {
  const r = {};
  const oidc = { type: "oidc", issuer: ISSUER, client: CLIENT, secret: SECRET };
  r.byMember = await setAuth(tokens.member, oidc);
  r.afterMember = (await getAuth(tokens.admin)).type;
  r.byAdmin = await setAuth(tokens.admin, oidc);
  const g = await getAuth(tokens.admin);
  r.read = [g.type, g.issuer === ISSUER, g.client === CLIENT, "secret" in g];
  r.signIn = await oidcSignIn("m3-admin");
  r.passwordUnderOidc = await password();
  r.memberBack = await setAuth(tokens.member, { type: "builtin" });
  r.restore = await setAuth(tokens.admin, { type: "builtin" });
  r.afterRestore = (await getAuth(tokens.member)).type;
  r.oidcAfterRestore = (await oidcSignIn("m3-admin")).start;
  r.passwordAfterRestore = await password();
  const pass = r.byMember === 403 && r.afterMember === "builtin" && r.byAdmin === 204
    && r.read.join() === "oidc,true,true,false"
    && r.signIn.start === 302 && r.signIn.toIdp && r.signIn.noCookie === 400 && r.signIn.callback === 201 && r.signIn.method === "Oidc" && r.signIn.account === state.admin
    && r.passwordUnderOidc === "WrongMethod" && r.memberBack === 403 && r.restore === 204 && r.afterRestore === "builtin"
    && r.oidcAfterRestore === 404 && r.passwordAfterRestore === "ok";
  if (pass) c.ok++; else c.fail.push(i);
  row(out, { i, pass, ...r });
  log(`BF-36 ${i}/${runs}: ${pass ? "pass" : "FAIL"} ${JSON.stringify(r)}`);
}
row(out, { summary: c });
log(`BF-36: ${JSON.stringify(c)}`);
