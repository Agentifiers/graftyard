// narrow-reads-2: the edge's per-isolate read caches never serve a stale answer past what DESIGN allows ("Follower
// read"; workers/edge/src/rcache.rs). On a live install, each check warms the caches with reads, changes the state,
// and then reads again:
//   1. new repository: a fresh repository under an owner whose `Authz` the isolate holds resolves at once (200, never
//      404), on REST and GraphQL;
//   2. grant opens access: a private repository the member cannot read (404), then `SetGrant` Read: 200 at once;
//   3. grant closed: `SetGrant` None, then 404 within READ_CACHE (10 s) plus one read's time, and never 200 after;
//   4. PAT revoked: 401 within READ_CACHE for reads, and at once for a write (a `POST` comment);
//   5. installation token revoked (`DELETE /installation/token`): 401 within READ_CACHE for reads.
// Prints one JSON line per check and exits 1 on any violation. Environment as tests/am1/hosted/lib.mjs.
//   node tests/nr2/stale.mjs
const { api, bed, ghs, must, pat, state, tokens, OP, sleep, secret } = await import("../am1/hosted/lib.mjs");
import { createHash } from "node:crypto";

const ORG = "m3org";
const BOUND = 10_000 + 3_000; // READ_CACHE, plus one read and the client's distance
const fails = [];
const out = (v) => { console.log(JSON.stringify(v)); if (!v.ok) fails.push(v.check); };
const issues = (repo, tok) => api("GET", `/api/v3/repos/${ORG}/${repo}/issues?per_page=1`, tok);
const gql = (repo, tok) => api("POST", "/api/graphql", tok, { query: `query { repository(owner: "${ORG}", name: "${repo}") { pullRequests(last: 1) { totalCount } } }` });
/** Reads with `tok` until `want(status)` or `ms` pass; returns [elapsed ms, statuses seen]. */
async function until(f, want, ms) {
  const t0 = Date.now(); const seen = [];
  for (;;) {
    const r = await f(); seen.push(r.status);
    if (want(r.status)) return [Date.now() - t0, seen];
    if (Date.now() - t0 > ms) return [null, seen];
    await sleep(250);
  }
}
async function mkrepo(name, priv) {
  for (let i = 0; i < 6; i++) {
    const r = await api("POST", `/api/v3/orgs/${ORG}/repos`, tokens.admin, { name, private: priv, auto_init: true });
    if (r.status === 201) return r.body.id;
    console.log(JSON.stringify({ create: name, status: r.status, message: r.body?.message ?? r.body }));
    await sleep(5_000);
  }
  throw new Error(`create ${name} failed`);
}
const tag = Date.now().toString(36);
const warm = async (tok, repo = "hot-a") => { for (let i = 0; i < 20; i++) await issues(repo, tok).catch(() => {}); };

// 1. A new repository resolves at once under a warm owner.
await warm(tokens.admin);
const a = `nr2-new-${tag}`;
const aId = await mkrepo(a, false);
const [r1, g1] = [await issues(a, tokens.admin), await gql(a, tokens.admin)];
out({ check: "new repository resolves at once", repo: aId, rest: r1.status, graphql: g1.status, data: !!g1.body?.data?.repository,
  ok: r1.status === 200 && g1.status === 200 && !!g1.body?.data?.repository });

// 2 and 3. A grant opens a private repository at once and closes it within READ_CACHE.
const b = `nr2-priv-${tag}`;
const bId = await mkrepo(b, true);
await warm(tokens.member);
const before = await issues(b, tokens.member);
if (before.status !== 404) out({ check: "member cannot read the private repository first", status: before.status, ok: false });
for (let i = 0; i < 10; i++) await issues(b, tokens.member); // the isolates hold the owner's Authz without the grant
const grant = (permission) => must(`authz:${state.org}`, { cred: OP, op: { SetGrant: { repo: bId, user: state.member, permission } } });
await grant("Read");
const opened = await issues(b, tokens.member);
out({ check: "grant opens access at once", status: opened.status, ok: opened.status === 200 });
for (let i = 0; i < 10; i++) await issues(b, tokens.member); // now the isolates hold the fold with the grant
await grant(null);
const t3 = Date.now();
const [closedAfter, seen3] = await until(() => issues(b, tokens.member), (s) => s === 404, BOUND + 5_000);
const after = []; for (let i = 0; i < 10; i++) after.push((await issues(b, tokens.member)).status);
out({ check: "removed grant closes within READ_CACHE", ms: closedAfter, bound: BOUND, seen: seen3.join(","), after: after.join(","),
  ok: closedAfter !== null && closedAfter <= BOUND && after.every((s) => s === 404) });

// 4. A revoked PAT: reads refused within READ_CACHE, a write at once.
const p = await pat(state.admin, "nr2 stale probe");
await warm(p);
const ok4 = (await issues(a, p)).status;
const raw = p.slice(4, 34);
const h = createHash("sha256").update("graftyard/token/").update(raw).digest();
await must(`tokens:${h.readUInt32BE(0) % 3072}`, { cred: OP, op: { Revoke: { hash: [...h] } } });
const w = await api("POST", `/api/v3/repos/${ORG}/${a}/issues`, p, { title: "nr2 revoked write" });
const [readRefused, seen4] = await until(() => issues(a, p), (s) => s === 401, BOUND + 5_000);
out({ check: "revoked PAT: write at once, reads within READ_CACHE", before: ok4, write: w.status, ms: readRefused, seen: seen4.join(","),
  ok: ok4 === 200 && w.status === 401 && readRefused !== null && readRefused <= BOUND });

// 5. A revoked installation token.
const t = await ghs();
const g = secret(t.body.token);
await warm(g);
const ok5 = (await issues(a, g)).status;
const del = await api("DELETE", "/api/v3/installation/token", g);
const [ghsRefused, seen5] = await until(() => issues(a, g), (s) => s === 401, BOUND + 5_000);
out({ check: "revoked ghs_: reads within READ_CACHE", before: ok5, delete: del.status, ms: ghsRefused, seen: seen5.join(","),
  ok: ok5 === 200 && del.status === 204 && ghsRefused !== null && ghsRefused <= BOUND });

console.log(JSON.stringify({ repos: [aId, bId], failed: fails }));
process.exit(fails.length ? 1 : 0);
