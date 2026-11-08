// BF-31 on graft-m2b (`build/BUILD-PLAN.md`, M3-WP2), each case `runs` times:
// - scope: m3-member mints a `github_pat_` for m3org/j4 with `contents: read` (`POST /api/graftyard/v1/user/tokens`).
//   It reads j4 (200), is refused on m3org/j4b outside its scope (404), and is refused a push and an issue on j4
//   (403: `contents: read` only). A `contents: write` one for j4 pushes there (control) and is refused on j4b.
// - pair: m3-member authorizes the App m3-ci (`/login/oauth/authorize`), and the code gives a `ghu_` and a `ghr_`.
//   The `ghu_` reads `/user`; a `ghr_` is no bearer (401). A refresh gives a new pair, after which the old `ghu_` is
//   401 and the old `ghr_` is `bad_refresh_token`. Revoking the new `ghr_` with `DELETE /applications/{client}/token`
//   makes its `ghu_` 401; on a third pair, revoking the `ghu_` makes its `ghr_` refresh fail.
// - delete: `DELETE /applications/{client}/token` with a wrong secret is 404 and the token still works; with the
//   right one it is 204, and the token is then 401 for a read and for a push.
// The App's client secret is read from <state>/app.secret and sent only in the Basic header and the form body.
//   node tests/m3/rows/bf31.mjs <runs> <out.jsonl>
import { readFileSync } from "node:fs";
import { BED, EDGE, OP, STATE, api, log, must, read, repoShard, row, secret, state, tokens } from "./lib.mjs";

const [runsArg, out] = process.argv.slice(2);
const runs = Number(runsArg ?? 10);
const CLIENT = state.client;
const SECRET = secret(readFileSync(`${STATE}/app.secret`, "utf8").trim());
const member = tokens.member;

// m3org/j4b, a second repository of the org outside the token's scope (idempotent).
if (!state.repo2) {
  const o = await must(`authz:${state.org}`, { cred: OP, op: { CreateRepo: { name: "j4b" } } }, "Taken");
  state.repo2 = o?.ok?.out?.Repo?.repo ?? (await read(`authz:${state.org}`, { repo: "j4b" }))?.repo?.repo;
}
await must(`names:${repoShard(state.repo2)}`, { cred: OP, op: { RepoOwner: { repo: state.repo2, owner: state.org } } });

const hex = (n) => n.toString(16).padStart(40, "0");
async function push(token, repo, ref) {
  const r = await fetch(`${EDGE}/_bed/push?repo=${repo}`, {
    method: "POST", signal: AbortSignal.timeout(180_000),
    headers: { "x-bed-token": BED, authorization: `token ${token}` },
    body: JSON.stringify({ ref, expect: null, new: hex(Date.now() * 1000 + Math.floor(Math.random() * 1000)) }),
  });
  if (r.status === 401) return 401;
  const v = await r.json().catch(() => ({}));
  // `Result<PushOutcome, _>` as serde writes it: `{"Ok": {"ok": ...}}` or `{"Ok": {"refused": {status}}}`.
  const o = v.outcome?.Ok ?? v.outcome;
  return o?.ok ? 200 : o?.refused?.status ?? `error ${String(v.error ?? JSON.stringify(v.outcome)).slice(0, 60)}`;
}
const form = (o) => new URLSearchParams(o).toString();
async function oauth(body) {
  const r = await fetch(`${EDGE}/login/oauth/access_token`, { method: "POST", signal: AbortSignal.timeout(180_000),
    headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" }, body: form(body) });
  const v = await r.json().catch(() => ({}));
  for (const k of ["access_token", "refresh_token"]) if (v[k]) secret(v[k]);
  return v;
}
async function pair() {
  const r = await fetch(`${EDGE}/login/oauth/authorize?${form({ client_id: CLIENT, redirect_uri: "http://localhost/cb", state: "s" })}`, {
    redirect: "manual", headers: { authorization: `token ${member}` }, signal: AbortSignal.timeout(180_000) });
  const code = new URL(r.headers.get("location") ?? "http://x/").searchParams.get("code");
  if (!code) return { error: `authorize ${r.status}` };
  secret(code);
  return oauth({ client_id: CLIENT, client_secret: SECRET, code });
}
const refresh = (t) => oauth({ client_id: CLIENT, client_secret: SECRET, grant_type: "refresh_token", refresh_token: t });
async function del(token, secretText) {
  const r = await fetch(`${EDGE}/api/v3/applications/${CLIENT}/token`, { method: "DELETE", signal: AbortSignal.timeout(180_000),
    headers: { authorization: `Basic ${Buffer.from(`${CLIENT}:${secretText}`).toString("base64")}`, "content-type": "application/json" },
    body: JSON.stringify({ access_token: token }) });
  return r.status;
}
const user = async (t) => (await api("GET", "/user", t)).status;

const c = { runs, scope: 0, pair: 0, del: 0, useAfterRevoke: 0, failures: [] };
for (let i = 1; i <= runs; i++) {
  // ---- scope ----
  const mint = async (level) => {
    const r = await fetch(`${EDGE}/api/graftyard/v1/user/tokens`, { method: "POST", signal: AbortSignal.timeout(180_000),
      headers: { authorization: `token ${member}`, "content-type": "application/json" },
      body: JSON.stringify({ owner: "m3org", repositories: ["j4"], permissions: { contents: level }, name: `bf31 ${level} ${i}` }) });
    const v = await r.json().catch(() => ({}));
    if (r.status !== 201) throw new Error(`mint ${level}: ${r.status} ${String(v.message).slice(0, 100)}`);
    return secret(v.token);
  };
  const ro = await mint("read");
  const rw = await mint("write");
  const s = {
    readIn: (await api("GET", "/repos/m3org/j4/issues", ro)).status,
    readOut: (await api("GET", "/repos/m3org/j4b/issues", ro)).status,
    pushIn: await push(ro, state.repo, `refs/heads/bf31/ro-${i}`),
    issueIn: (await api("POST", "/repos/m3org/j4/labels", ro, { name: `bf31-${Date.now()}`, color: "ededed" })).status,
    rwPushIn: await push(rw, state.repo, `refs/heads/bf31/rw-${i}`),
    rwPushOut: await push(rw, state.repo2, `refs/heads/bf31/rw-${i}`),
    classicPushOut: await push(member, state.repo2, `refs/heads/bf31/classic-${i}`),
  };
  const scopeOk = s.readIn === 200 && s.readOut === 404 && s.pushIn === 403 && s.issueIn === 403 && s.rwPushIn === 200
    && [403, 404].includes(s.rwPushOut) && s.classicPushOut === 200;
  // ---- pair ----
  const a = await pair();
  const p = {
    kinds: `${a.access_token?.slice(0, 4)}${a.refresh_token?.slice(0, 4)}`,
    ghu: await user(a.access_token), ghrBearer: await user(a.refresh_token),
  };
  const b = await refresh(a.refresh_token);
  p.refreshed = !!b.access_token && b.access_token !== a.access_token;
  p.oldGhu = await user(a.access_token);
  p.oldGhr = (await refresh(a.refresh_token)).error ?? "TOKEN";
  p.newGhu = await user(b.access_token);
  p.revokeGhr = await del(b.refresh_token, SECRET);
  p.pairedGhu = await user(b.access_token);
  p.pairedGhuPush = await push(b.access_token, state.repo, `refs/heads/bf31/u-${i}`);
  const d3 = await pair();
  p.revokeGhu = await del(d3.access_token, SECRET);
  p.pairedGhr = (await refresh(d3.refresh_token)).error ?? "TOKEN";
  const pairOk = p.kinds === "ghu_ghr_" && p.ghu === 200 && p.ghrBearer === 401 && p.refreshed && p.oldGhu === 401
    && p.oldGhr === "bad_refresh_token" && p.newGhu === 200 && p.revokeGhr === 204 && p.pairedGhu === 401
    && p.pairedGhuPush === 401 && p.revokeGhu === 204 && p.pairedGhr === "bad_refresh_token";
  // ---- DELETE ----
  const e = await pair();
  const d = { wrongSecret: await del(e.access_token, "not-the-secret"), stillWorks: await user(e.access_token) };
  d.del = await del(e.access_token, SECRET);
  d.after = await user(e.access_token);
  d.afterPush = await push(e.access_token, state.repo, `refs/heads/bf31/d-${i}`);
  d.again = await del(e.access_token, SECRET);
  const delOk = d.wrongSecret === 404 && d.stillWorks === 200 && d.del === 204 && d.after === 401 && d.afterPush === 401 && d.again === 204;
  // Any use after revocation, the fail value.
  const used = [p.oldGhu, p.pairedGhu, p.pairedGhuPush, d.after, d.afterPush].filter((x) => x === 200).length
    + [p.oldGhr, p.pairedGhr].filter((x) => x === "TOKEN").length;
  c.useAfterRevoke += used;
  if (scopeOk) c.scope++;
  if (pairOk) c.pair++;
  if (delOk) c.del++;
  if (!scopeOk || !pairOk || !delOk) c.failures.push({ i, s, p, d });
  row(out, { i, scope: s, pair: p, delete: d, ok: { scopeOk, pairOk, delOk } });
  log(`BF-31 ${i}/${runs}: scope ${scopeOk} pair ${pairOk} delete ${delOk}`);
}
row(out, { summary: { ...c, failures: c.failures.length } });
log(`BF-31: ${JSON.stringify({ ...c, failures: c.failures.length })}`);
