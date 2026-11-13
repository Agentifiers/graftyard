// BF-67 on graft-m4a, after seed.mjs. Two halves:
//
// 1. Stars: `/stargazers` of m3org/wp2-stars lists every seeded account (FIRST .. FIRST+N-1) and `stargazers_count`
//    equals the list, read through the edge 100 to a page with the star media type. Then the repository is refolded:
//    the product fold audit (`/fold {audit: {start, force}}` on the leader) refolds the chain from its snapshots and log
//    into scratch tables, and every voter's metadata digest at that index must equal the refold's; `/stargazers` is
//    listed again after.
// 2. The Inbox: m3-member (subscribed to wp2-bf5 and a thread of it from BF-5) reads one thread and marks another
//    done; its Inbox is snapshotted. Then a new event on every `sources` chain (a comment on each thread), after
//    which the Inbox is deleted and rebuilt from the snapshot plus `sources`. Pass: the rebuilt `/notifications?all=true`
//    equals the live one, read and done kept, and every new comment present.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp2/hosted/bf67.mjs <N>
import { api, leaderOf, log, member, row, sleep, state, tokens } from "../../../am1/hosted/lib.mjs";

const N = Number(process.argv[2] ?? 10000);
// `stars`, `inbox`, or both (default): the halves run apart when a site day comes between them.
const PART = process.argv[3] ?? "both";
const FIRST = 7300000;
const out = new URL("../raw/bf67.jsonl", import.meta.url).pathname;
const STARS = "m3org/wp2-stars", WATCHED = "m3org/wp2-bf5";
const admin = tokens.admin, watcher = tokens.member;

// ---- 1. stars ----
let live = { listed: N, missing: 0, distinct: N, count: N, ghosts: 0 }, after = { listed: N, missing: 0 }, metaSame = true, coreSame = true;
if (PART !== "inbox") {
async function listAll() {
  const ids = [], logins = [];
  let ghosts = 0, ms = 0;
  for (let page = 1; ; page++) {
    const r = await api("GET", `/api/v3/repos/${STARS}/stargazers?per_page=100&page=${page}`, admin, undefined, { accept: "application/vnd.github.star+json" });
    if (r.status !== 200) throw new Error(`page ${page}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
    ms = Math.max(ms, r.ms);
    for (const s of r.body) { ids.push(s.user.id); logins.push(s.user.login); if (s.user.login === "ghost") ghosts++; }
    if (!(r.headers.link ?? "").includes('rel="next"')) break;
  }
  const want = new Set(Array.from({ length: N }, (_, i) => FIRST + i));
  const missing = [...want].filter((a) => !ids.includes(a));
  const named = logins.filter((l, i) => l === `wp2-s${ids[i] - FIRST}`).length;
  const count = (await api("GET", `/api/v3/repos/${STARS}`, admin)).body.stargazers_count;
  return { listed: ids.length, distinct: new Set(ids).size, missing: missing.length, missingSample: missing.slice(0, 10), ghosts, named, count, pageMsMax: ms };
}

// Counts lags the seed by a round or two: wait until the count reaches N or stops moving.
let last = -1;
for (let same = 0, t0 = Date.now(); same < 6 && Date.now() - t0 < 600_000; await sleep(5000)) {
  const c = (await api("GET", `/api/v3/repos/${STARS}`, admin)).body.stargazers_count;
  same = c === last ? same + 1 : 0;
  last = c;
  if (c >= N) break;
}
live = await listAll();
log(`live: ${JSON.stringify(live)}`);
row(out, { step: "stars-live", ...live });

const chain = `repo:${(await api("GET", `/api/v3/repos/${STARS}`, admin)).body.id}`;
const lead = await leaderOf(chain);
const fold = async (b) => (await member(chain, lead, "/fold", b, 60_000)).v;
const prior = (await fold({ audit: {} }))?.last?.index ?? 0;
const started = await fold({ audit: { start: true, force: true } });
log(`refold started: ${JSON.stringify(started).slice(0, 200)}`);
let rep;
for (const t0 = Date.now(); Date.now() - t0 < 600_000; await sleep(3000)) {
  const v = await fold({ audit: {} });
  if (!v?.running && (v?.last?.index ?? 0) > 0 && (v.last.index !== prior || v.last.at !== started?.last?.at)) { rep = v.last; break; }
}
const at = rep?.index;
const digests = [];
for (const s of [0, 1, 2]) {
  const v = (await member(chain, s, "/fold", { at }, 60_000)).v;
  digests.push({ site: s, core: v?.core?.at, meta: v?.metadata?.at });
}
metaSame = digests.every((d) => d.meta && d.meta === rep?.metadata);
coreSame = digests.every((d) => d.core && d.core === rep?.core);
log(`refold at ${at}: refold meta ${rep?.metadata} core ${rep?.core}; voters ${JSON.stringify(digests)}; meta equal ${metaSame}, core equal ${coreSame}`);
after = await listAll();
log(`after refold: ${JSON.stringify(after)}`);
row(out, { step: "refold", report: rep, digests, metaSame, coreSame, after });
}

// ---- 2. the Inbox ----
let same = true, emptied = 0;
if (PART !== "stars") {
const gql = async (q, v) => {
  const r = await api("POST", "/api/graphql", admin, { query: q, variables: v });
  if (r.status !== 200 || r.body?.errors) throw new Error(`graphql ${JSON.stringify(r.body).slice(0, 200)}`);
  return r.body.data;
};
const view = async () => (await api("GET", "/api/v3/notifications?all=true&per_page=50", watcher)).body;
const shape = (l) => l.map((x) => ({ id: x.id, unread: x.unread, reason: x.reason, last_read_at: x.last_read_at, updated_at: x.updated_at })).sort((a, b) => a.id.localeCompare(b.id));
const before = await view();
if (before.length < 3) throw new Error(`the watcher has ${before.length} threads; run bf5.mjs first`);
const [readT, doneT] = before;
await api("PATCH", `/api/v3/notifications/threads/${readT.id}`, watcher);
await api("DELETE", `/api/v3/notifications/threads/${doneT.id}`, watcher);
const ichain = `identity:${state.member}`;
const ilead = await leaderOf(ichain);
const inbox = async (op) => (await member(ichain, ilead, "/inbox", { op }, 120_000)).v;
await sleep(2000);
const snap = await inbox("snapshot");
log(`snapshot ${JSON.stringify(snap).slice(0, 120)}; read ${readT.id}, done ${doneT.id}`);
// A new event on the `sources` chain after the snapshot: a comment on every thread.
for (const t of before) {
  const n = Number(t.subject.url.split("/").pop());
  const node = (await gql("query($o: String!, $n: String!, $i: Int!) { repository(owner: $o, name: $n) { issue(number: $i) { id } } }", { o: "m3org", n: "wp2-bf5", i: n })).repository.issue.id;
  await gql("mutation($s: ID!) { addComment(input: {subjectId: $s, body: \"BF-67: after the snapshot\"}) { clientMutationId } }", { s: node });
}
let liveI;
for (const t0 = Date.now(); Date.now() - t0 < 60_000; await sleep(1000)) {
  liveI = shape(await view());
  const moved = liveI.filter((x) => before.some((b) => b.id === x.id && x.updated_at > b.updated_at)).length;
  if (moved >= before.length - 1) break; // the done thread stays hidden until a newer notice, then reappears unread
}
await inbox("delete");
emptied = (await view()).length;
const t1 = Date.now();
const rebuilt = await inbox("rebuild");
const rebuildMs = Date.now() - t1;
const afterI = shape(await view());
same = JSON.stringify(afterI) === JSON.stringify(liveI);
log(`live inbox ${JSON.stringify(liveI)}`);
log(`deleted (${emptied} left), rebuilt in ${rebuildMs} ms: ${JSON.stringify(rebuilt).slice(0, 200)}`);
log(`rebuilt inbox ${JSON.stringify(afterI)}; same ${same}`);
row(out, { step: "inbox", read: readT.id, done: doneT.id, snapshot: snap, live: liveI, emptied, rebuildMs, rebuilt, after: afterI, same });
}

const pass = live.listed === N && live.missing === 0 && live.distinct === N && live.count === N && live.ghosts === 0
  && after.listed === N && after.missing === 0 && metaSame && coreSame && same && emptied === 0;
row(out, { step: "verdict", part: PART, pass });
log(`BF-67 (${PART}) ${pass ? "PASS" : "FAIL"}`);
