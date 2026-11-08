// The activity feed hosted on m3org/j4 (`repo:1048576`), through graft-m1-edge with real member routing:
//   lf3     a cursor of another group with wait=40, then 33 `paths`: 410 with `oldest_cursor`, wait clamped to 25
//           (the held poll returns by 25 s + slack), 422
//   l015    anonymous, a token without the repository, and an over-limit (FEED_POLLS) poll: 401/404/429, then every
//           member's `leaderPolls`
//   lf12    N held polls with `paths` (default 5,000) from M tokens, while pushes commit; per member held/peak/refused
//           from `/status`, every 429 with `Retry-After`, commit p99 of the pushes; then `/learners {"feed": true}` on
//           the leader and the same again
//   lf13    1,000 polls held on one follower, then its member calls blackholed (site fault) for 5 s so it falls
//           behind, then caught up: held polls return within 1 s with their cursor and no events, new ones refused
//           while behind, no event lost
//   events  latency of each pushed event to 500 long-pollers (L0-12's feed line)
// Rows go to tests/am1/raw/feed-<row>.jsonl.
//   node tests/am1/hosted/feed.mjs <row> [n]
import { api, bedGet, ghs, log, member, pat, row, sleep, state, statuses } from "./lib.mjs";

const [which, nArg] = process.argv.slice(2);
const RAW = new URL("../raw/", import.meta.url).pathname;
const OUT = `${RAW}feed-${which}.jsonl`;
const R = "m3org/j4";
const CHAIN = `repo:${state.repo}`;
const FEED = `/api/graftyard/v1/repos/${R}/activity`;
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const feedStatus = async () => (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, feed: s.feed, stalledSince: s.stalledSince }));

async function cursorNow(tok) {
  const r = await api("GET", `${FEED}?wait=0`, tok);
  return { status: r.status, cursor: r.body?.cursor, body: r.body };
}

if (which === "lf3") {
  const t = (await ghs({})).body.token;
  const now = await cursorNow(t);
  row(OUT, { step: "cursor now", status: now.status, cursor: now.cursor, msg: now.body?.message });
  // Another group's cursor: version 1, group j4 + 1, index 1, in the cursor's own encoding (`FeedCursor::encode`).
  const cb = Buffer.alloc(17); cb[0] = 1; cb.writeBigInt64BE(BigInt(state.repo + 1), 1); cb.writeBigUInt64BE(1n, 9);
  const other = cb.toString("base64url");
  const own = now.cursor;
  const t0 = Date.now();
  const a = await api("GET", `${FEED}?cursor=${other}&wait=40`, t);
  row(OUT, { step: "foreign cursor wait=40", status: a.status, oldest: a.body?.oldest_cursor, ms: Date.now() - t0, own });
  const t1 = Date.now();
  const b = await api("GET", `${FEED}?wait=40${now.cursor ? `&cursor=${now.cursor}` : ""}&paths=nothing/here/**`, t);
  row(OUT, { step: "own cursor wait=40 (clamped)", status: b.status, ms: Date.now() - t1, events: b.body?.events?.length });
  const p33 = Array.from({ length: 33 }, (_, i) => `paths=d${i}/**`).join("&");
  const c = await api("GET", `${FEED}?wait=1&${p33}`, t);
  row(OUT, { step: "33 paths", status: c.status, msg: c.body?.message, errors: c.body?.errors });
  const p32 = Array.from({ length: 32 }, (_, i) => `paths=d${i}/**`).join("&");
  const d = await api("GET", `${FEED}?wait=1&${p32}`, t);
  row(OUT, { step: "32 paths (control)", status: d.status });
  log(JSON.stringify({ foreign: a.status, oldest: !!a.body?.oldest_cursor, clampedMs: Date.now() - t1, p33: c.status, p32: d.status }));
}

if (which === "l015") {
  const anon = await api("GET", `${FEED}?wait=0`, null);
  const meta = await ghs({ permissions: { metadata: "read" } });
  const mo = meta.status === 201 ? await api("GET", `${FEED}?wait=0`, meta.body.token) : { status: `mint ${meta.status}` };
  // Over limit: FEED_POLLS.0 = 2 per token held at once; a third from the same token while two are held.
  const t = (await ghs({})).body.token;
  const held = [api("GET", `${FEED}?wait=20&paths=nothing/**`, t), api("GET", `${FEED}?wait=20&paths=nothing/**`, t)];
  await sleep(3000);
  const third = await api("GET", `${FEED}?wait=20&paths=nothing/**`, t);
  await Promise.all(held);
  const st = await feedStatus();
  const v = { anonymous: anon.status, metadataOnly: mo.status, overLimit: third.status, retryAfter: third.headers?.["retry-after"], members: st };
  row(OUT, v);
  log(JSON.stringify(v).slice(0, 600));
}

/** N held polls with `paths` from `tokens` (2 per token); returns status counts, Retry-After presence, latencies. */
async function hold(n, tokens, wait = 20) {
  const res = { n, status: {}, noRetryAfter: 0, ms: [] };
  const one = async (k) => {
    const tok = tokens[Math.floor(k / 2) % tokens.length];
    const r = await api("GET", `${FEED}?wait=${wait}&paths=held/${k % 50}/**`, tok);
    res.status[r.status] = (res.status[r.status] ?? 0) + 1;
    if (r.status === 429 && !r.headers?.["retry-after"]) res.noRetryAfter++;
    res.ms.push(r.ms);
  };
  await Promise.all(Array.from({ length: n }, (_, k) => sleep((k % 500) * 4).then(() => one(k))));
  return res;
}

async function tokensFor(n) {
  // FEED_POLLS = (2 per token, 50 per IP): one orb IP holds at most 50 per member, so a 5,000-poll run from one
  // address is capped by the per-IP limit before FEED_HELD_MAX. Recorded, not hidden.
  const out = [];
  for (let i = 0; i < n; i++) out.push(await pat(state.member, `am1 feed ${i}`));
  return out;
}

if (which === "lf12") {
  const n = Number(nArg ?? 5000);
  const toks = await tokensFor(Math.ceil(n / 2 / 10));
  const before = await feedStatus();
  const t0 = Date.now();
  const r1 = await hold(n, toks);
  const mid = await feedStatus();
  row(OUT, { phase: "before place_learners", ms: Date.now() - t0, status: r1.status, noRetryAfter: r1.noRetryAfter, p99: pct(r1.ms, 0.99), before, after: mid });
  log(`lf12 before: ${JSON.stringify(r1.status)} noRetryAfter=${r1.noRetryAfter} members=${JSON.stringify(mid.map((m) => [m.site, m.role, m.feed?.peak, m.feed?.refused, m.feed?.leaderPolls]))}`);
  const leader = mid.find((m) => m.role === "leader")?.site;
  const placed = await member(CHAIN, leader, "/learners", { load: { reads: [] }, feed: true }, 120_000);
  row(OUT, { phase: "place_learners", leader, placed: placed.v ?? placed });
  log(`place_learners: ${JSON.stringify(placed.v ?? placed).slice(0, 500)}`);
  await sleep(70_000); // the edge's member cache lives 60 s
  const r2 = await hold(n, toks);
  const after = await feedStatus();
  row(OUT, { phase: "after place_learners", status: r2.status, noRetryAfter: r2.noRetryAfter, p99: pct(r2.ms, 0.99), after });
  log(`lf12 after: ${JSON.stringify(r2.status)} members=${JSON.stringify(after.map((m) => [m.site, m.role, m.feed?.peak, m.feed?.refused, m.feed?.polls]))}`);
}

if (which === "status") log(JSON.stringify(await feedStatus()));
if (which === "fault-off") log(JSON.stringify(await bedGet("/_bed/fault?off=1")));
