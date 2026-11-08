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
const R = `m3org/${process.env.FEED_REPO ?? "j4"}`;
const CHAIN = `repo:${process.env.FEED_REPO_ID ?? state.repo}`;
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

/** A poll straight to one member's `/feed` (as the edge sends it), with its own token and IP hashes. */
async function direct(site, k, wait, extra = {}) {
  const t0 = Date.now();
  try {
    const r = await member(CHAIN, site, "/feed", { cursor: extra.cursor ?? null, wait_s: wait, paths: extra.paths ?? [`held/${k % 50}/**`],
      learner_answers: false, token: `t${k}`, ip: `ip${k}`, ...extra.body }, (wait + 10) * 1000, extra.name);
    return { admit: r.v?.admit ?? `status ${r.status}`, events: r.v?.events?.length ?? 0, cursor: r.v?.cursor, ms: Date.now() - t0, stopped: !!r.v?.stopped };
  } catch (e) { return { admit: `error ${String(e).slice(0, 60)}`, ms: Date.now() - t0 }; }
}

/** Commits through the edge while polls are held: PATCHes of one task's summary, `rate` a second; client latency. */
async function commits(app, id, secs, rate = 2) {
  const ms = [], st = {};
  const end = Date.now() + secs * 1000;
  let k = 0;
  const inflight = [];
  while (Date.now() < end) {
    inflight.push(api("PATCH", `/api/graftyard/v1/repos/${R}/tasks/${id}`, app, { summary: `lf12 ${k++}` }).then((r) => { ms.push(r.ms); st[r.status] = (st[r.status] ?? 0) + 1; }));
    await sleep(1000 / rate);
  }
  await Promise.all(inflight);
  return { n: ms.length, status: st, p50: pct(ms, 0.5), p99: pct(ms, 0.99), max: Math.max(...ms) };
}

if (which === "lf12") {
  const n = Number(nArg ?? 5000);
  const app = (await ghs({})).body.token;
  const task = await api("POST", `/api/graftyard/v1/repos/${R}/tasks`, app, { kind: "external", session: `lf12-${Date.now().toString(36)}`, summary: "lf12" });
  const id = task.body?.id;
  // Phase A, through the edge: one client IP, so FEED_POLLS' 50 per IP per member binds before FEED_HELD_MAX.
  const toks = await tokensFor(20);
  const ra = await hold(400, toks);
  const sa = await feedStatus();
  row(OUT, { phase: "A edge, one IP", polls: 400, status: ra.status, noRetryAfter: ra.noRetryAfter, members: sa });
  log(`A: ${JSON.stringify(ra.status)} noRetryAfter=${ra.noRetryAfter} ${JSON.stringify(sa.map((m) => [m.site, m.role, m.feed?.peak, m.feed?.refused, m.feed?.leaderPolls]))}`);
  // Phase B, straight to members: n polls with distinct token and IP hashes over the voters, commits running.
  const st = await feedStatus();
  // The followers, as the edge picks them (it leaves out the voter whose status says it leads).
  const voters = st.filter((m) => m.role === "follower").map((m) => m.site);
  const polls = Array.from({ length: n }, (_, k) => sleep((k % 1000) * 3).then(() => direct(voters[k % voters.length], k, 25)));
  await sleep(5_000);
  const during = commits(app, id, 25);
  const [pb, cb] = await Promise.all([Promise.all(polls), during]);
  const sb = await feedStatus();
  const bySite = {};
  pb.forEach((r, k) => { const s = voters[k % voters.length]; bySite[s] ??= {}; bySite[s][r.admit] = (bySite[s][r.admit] ?? 0) + 1; });
  row(OUT, { phase: "B members, before place_learners", polls: n, bySite, commits: cb, members: sb });
  log(`B: ${JSON.stringify(bySite)} commits ${JSON.stringify(cb)} ${JSON.stringify(sb.map((m) => [m.site, m.role, m.feed?.peak, m.feed?.refused]))}`);
  const leader = sb.find((m) => m.role === "leader")?.site;
  const placed = await member(CHAIN, leader, "/learners", { load: { reads: [] }, feed: true }, 120_000);
  row(OUT, { phase: "place_learners", leader, placed: placed.v ?? placed });
  log(`place_learners: ${JSON.stringify(placed.v ?? placed).slice(0, 600)}`);
  await sleep(70_000);
  const st2 = await statuses(CHAIN, 3);
  const learners = (placed.v?.learners ?? []).map((l) => ({ site: l.site, name: l.doName }));
  // Phase C: the same n polls to learners (the members the edge now tries first), commits running; then through the
  // edge, where followers must see none.
  const pc = Array.from({ length: n }, (_, k) => sleep((k % 1000) * 3).then(() => {
    const l = learners[k % Math.max(1, learners.length)];
    return l ? direct(l.site, k, 25, { name: l.name }) : { admit: "no learner" };
  }));
  await sleep(5_000);
  const [rc, cc] = await Promise.all([Promise.all(pc), commits(app, id, 25)]);
  const byL = {};
  rc.forEach((r, k) => { const l = learners[k % Math.max(1, learners.length)]?.name ?? "-"; byL[l] ??= {}; byL[l][r.admit] = (byL[l][r.admit] ?? 0) + 1; });
  const fBefore = (await feedStatus()).map((m) => [m.site, m.feed?.polls]);
  const rd = await hold(200, toks);
  const fAfter = (await feedStatus()).map((m) => [m.site, m.feed?.polls]);
  row(OUT, { phase: "C after place_learners", byLearner: byL, commits: cc, edge: rd.status, voterPollsBefore: fBefore, voterPollsAfter: fAfter, voters: st2.map((m) => [m.site, m.role]) });
  log(`C: ${JSON.stringify(byL)} commits ${JSON.stringify(cc)} edge ${JSON.stringify(rd.status)} voter polls ${JSON.stringify(fBefore)} -> ${JSON.stringify(fAfter)}`);
}

if (which === "lf13") {
  // 1,000 polls held on one follower, then its site's member calls fail for 5 s while commits run, so its next
  // AppendEntries leaves a gap.
  const app = (await ghs({})).body.token;
  const task = await api("POST", `/api/graftyard/v1/repos/${R}/tasks`, app, { kind: "external", session: `lf13-${Date.now().toString(36)}`, summary: "lf13" });
  const st = await feedStatus();
  const f = st.find((m) => m.role === "follower")?.site;
  const held = Array.from({ length: 1000 }, (_, k) => sleep(k * 2).then(() => direct(f, k, 25)));
  await sleep(6_000);
  const s1 = await feedStatus();
  row(OUT, { step: "held", follower: f, members: s1 });
  const t0 = Date.now();
  const on = await bedGet(`/_bed/fault?site=${f}&for_ms=5000`);
  const during = commits(app, task.body.id, 8, 2);
  // New polls while it is behind (after the fault lifts, before it catches up), sampled every 300 ms.
  const probes = [];
  await sleep(5_200);
  for (let i = 0; i < 20; i++) { probes.push({ at: Date.now() - t0, ...(await direct(f, 5000 + i, 0)) }); await sleep(300); }
  const res = await Promise.all(held);
  const c = await during;
  const ret = res.map((r) => r.ms);
  row(OUT, { step: "after", fault: on.status, commits: c, held: res.length, admits: res.reduce((m, r) => ((m[r.admit] = (m[r.admit] ?? 0) + 1), m), {}),
    stopped: res.filter((r) => r.stopped).length, withEvents: res.filter((r) => r.events > 0).length, returnMs: { p50: pct(ret, 0.5), p99: pct(ret, 0.99) }, probes, members: await feedStatus() });
  log(`lf13: stopped ${res.filter((r) => r.stopped).length}/1000, events ${res.filter((r) => r.events > 0).length}, probes ${JSON.stringify(probes.map((p) => [p.at, p.admit]))}`);
}

if (which === "status") log(JSON.stringify(await feedStatus()));
if (which === "fault-off") log(JSON.stringify(await bedGet("/_bed/fault?off=1")));
