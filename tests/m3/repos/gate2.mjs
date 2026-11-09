// The edge read gate after 932cc33 (a refused REST credential is 401 "Bad credentials" before the gate) on graft-m2b,
// <runs> rounds. Two fresh repositories by m3-admin (`POST /orgs/m3org/repos`, auto_init): one private, one public.
// Each round makes a new PAT for m3-member (an org member, which reads both) and:
//   - reads the private repository with it (200), revokes it (`DELETE /applications/{gh}/token`, 204), waits 11 s,
//     then `GET /repos/m3org/<private>` (want 401 "Bad credentials") and git `info/refs` (want 401);
//   - with the same revoked token, `GET /repos/m3org/<public>` (want 401);
//   - with a well-formed token no account holds, `GET` on both (want 401);
//   - controls: anonymous `GET` on the public repository (200) and the private one (404).
// Tokens ride in headers only; nothing token-shaped is logged (`lib.mjs`'s `scan`).
//   node tests/m3/repos/gate2.mjs <runs> <out.jsonl>
import { randomBytes } from "node:crypto";
import { EDGE, api, log, pat, row, secret, sleep, state, tokens } from "../rows/lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 12);
const GH_CLIENT = "178c6fc778ccc68e1d6a";
const tag = Date.now().toString(36);
const priv = `gate2-priv-${tag}`, pub = `gate2-pub-${tag}`;
for (const [n, p] of [[priv, true], [pub, false]]) {
  const r = await api("POST", "/orgs/m3org/repos", tokens.admin, { name: n, private: p, auto_init: true });
  if (r.status !== 201) throw new Error(`create ${n}: ${r.status}`);
}
row(out, { setup: { priv, pub } });
const B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const unknown = () => secret(`ghp_${[...randomBytes(36)].map((b) => B62[b % 62]).join("")}`);
/** The status and `message` of a REST read; `token` null is anonymous. */
const get = async (path, token) => {
  const r = await api("GET", path, token);
  return `${r.status}${r.status >= 400 ? ` ${r.body?.message}` : ""}`;
};
const git = async (token) => {
  const h = token ? { authorization: `Basic ${Buffer.from(`x:${token}`).toString("base64")}` } : {};
  const r = await fetch(`${EDGE}/m3org/${priv}.git/info/refs?service=git-upload-pack`, { headers: h, signal: AbortSignal.timeout(120_000) });
  await r.arrayBuffer();
  return `${r.status}`;
};
const counts = {};
const tally = (k, v) => { counts[k] ??= {}; counts[k][v] = (counts[k][v] ?? 0) + 1; };
for (let i = 1; i <= runs; i++) {
  const p = await pat(state.member, `gate2 ${i}`);
  const r = { i };
  r.before = await get(`/repos/m3org/${priv}`, p.text);
  r.beforeGit = await git(p.text);
  const rv = await fetch(`${EDGE}/api/v3/applications/${GH_CLIENT}/token`, {
    method: "DELETE", signal: AbortSignal.timeout(120_000),
    headers: { authorization: `Basic ${Buffer.from(`${GH_CLIENT}:x`).toString("base64")}`, "content-type": "application/json" },
    body: JSON.stringify({ access_token: p.text }),
  });
  r.revoke = rv.status;
  await sleep(11_000);
  r.revokedPrivate = await get(`/repos/m3org/${priv}`, p.text);
  r.revokedGit = await git(p.text);
  r.revokedPublic = await get(`/repos/m3org/${pub}`, p.text);
  const u = unknown();
  r.unknownPrivate = await get(`/repos/m3org/${priv}`, u);
  r.unknownPublic = await get(`/repos/m3org/${pub}`, u);
  r.anonPublic = await get(`/repos/m3org/${pub}`, null);
  r.anonPrivate = await get(`/repos/m3org/${priv}`, null);
  for (const [k, v] of Object.entries(r)) if (k !== "i") tally(k, v);
  row(out, r);
  log(`gate2 ${i}/${runs}: ${JSON.stringify(r)}`);
}
row(out, { summary: counts });
log(`summary: ${JSON.stringify(counts)}`);
