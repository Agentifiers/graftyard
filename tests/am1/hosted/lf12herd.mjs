// LF-12's cause, isolated (am1-fix): commits run at 2/s on `FEED_REPO_ID` for 50 s; at 10 s a burst of N polls
// (default 3,000, wait 5 s) goes to the group's learners while their owner `Authz` is stale (no poll for 60 s), and at
// 30 s the same burst again while it is fresh. Per window: client commit latency, the leader's `commitMs`, each
// entry's slowest dep read from the leader's `/deps_log`, and the `/read`s the owner's Authz members took.
// Rows: tests/am1/raw/lf12herd.jsonl.
//   node tests/am1/hosted/lf12herd.mjs [n] [spread ms]
import { api, ghs, log, member, row, sleep, statuses } from "./lib.mjs";

const N = Number(process.argv[2] ?? 3000);
// Spread of the burst in ms (default 300): 0 sends every poll at once, as a fresh learner set meets its first polls.
const SPREAD = Number(process.argv[3] ?? 300);
const OUT = new URL("../raw/lf12herd.jsonl", import.meta.url).pathname;
const R = `m3org/${process.env.FEED_REPO ?? "lf1-4"}`;
const CHAIN = `repo:${process.env.FEED_REPO_ID ?? 1048581}`;
const OWNER = `authz:${process.env.OWNER_ID ?? 7100001}`;
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const sum = (xs) => ({ n: xs.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99), max: xs.length ? Math.max(...xs) : null });
const reads = async (reset) => Promise.all([0, 1, 2].map(async (s) => {
  try { return (await member(OWNER, s, `/stats${reset ? "?reset=1" : ""}`, undefined, 15_000)).v.inbound?.["/read"] ?? 0; } catch { return null; }
}));

const st = await statuses(CHAIN);
const cfg = st.find((s) => s.config)?.config?.config?.normal;
const learners = (cfg?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name }));
if (!learners.length) throw new Error("no learners: run place_learners first");
const app = (await ghs({})).body.token;
const task = await api("POST", `/api/graftyard/v1/repos/${R}/tasks`, app, { kind: "external", session: `herd-${Date.now().toString(36)}`, summary: "lf12herd" });
const id = task.body?.id;
// Warm the leader, then wait out the owner's 60 s refresh on every learner.
for (let i = 0; i < 6; i++) { await api("PATCH", `/api/graftyard/v1/repos/${R}/tasks/${id}`, app, { summary: `warm ${i}` }); await sleep(500); }
log(`learners ${learners.length}; waiting 65 s for the owner to go stale`);
await sleep(65_000);
const leader = (await statuses(CHAIN)).find((s) => s.role === "leader")?.site;
await member(CHAIN, leader, "/stats?reset=1", undefined, 15_000);
await reads(true);
const t0 = Date.now();
const done = [];
const end = t0 + 50_000;
let k = 0;
const inflight = [];
const commitsLoop = (async () => {
  while (Date.now() < end) {
    const s = Date.now();
    inflight.push(api("PATCH", `/api/graftyard/v1/repos/${R}/tasks/${id}`, app, { summary: `herd ${k++}` }).then((r) => done.push({ s: s - t0, ms: r.ms, status: r.status })));
    await sleep(500);
  }
  await Promise.all(inflight);
})();
const burst = async (tag) => {
  const before = await reads(false);
  const r = await Promise.all(Array.from({ length: N }, (_, j) => sleep(SPREAD ? j % SPREAD : 0).then(() => {
    const l = learners[j % learners.length];
    return member(CHAIN, l.site, "/feed", { cursor: null, wait_s: 5, paths: [`herd/${j % 50}/**`], learner_answers: false, token: `${tag}${j}`, ip: `${tag}ip${j}` }, 20_000, l.name)
      .then((x) => x.v?.admit ?? `status ${x.status}`).catch(() => "error");
  })));
  const after = await reads(false);
  return { tag, admits: r.reduce((m, a) => ((m[a] = (m[a] ?? 0) + 1), m), {}), ownerReads: after.map((a, i) => (a ?? 0) - (before[i] ?? 0)) };
};
await sleep(10_000);
const stale = await burst("stale");
await sleep(Math.max(0, t0 + 30_000 - Date.now()));
const fresh = await burst("fresh");
await commitsLoop;
const stats = (await member(CHAIN, leader, "/stats", undefined, 15_000)).v;
const deps = ((await member(CHAIN, leader, "/deps_log", undefined, 30_000)).v ?? []).filter((r) => r.at >= t0);
const win = (a, b) => ({
  client: sum(done.filter((d) => d.s >= a && d.s < b).map((d) => d.ms)),
  depSlowest: sum(deps.filter((r) => r.at - t0 >= a && r.at - t0 < b).map((r) => Math.max(...r.reads.map((x) => x.ms ?? 0)))),
  ownerDep: sum(deps.filter((r) => r.at - t0 >= a && r.at - t0 < b).map((r) => r.reads.find((x) => String(x.chain) === OWNER)?.ms ?? 0)),
});
const v = { n: N, learners: learners.length, leader, commitMs: sum(stats.commitMs ?? []), statuses: done.reduce((m, d) => ((m[d.status] = (m[d.status] ?? 0) + 1), m), {}),
  quiet: win(0, 10_000), staleBurst: { ...stale, ...win(10_000, 20_000) }, between: win(20_000, 30_000), freshBurst: { ...fresh, ...win(30_000, 40_000) }, after: win(40_000, 60_000) };
row(OUT, v);
log(JSON.stringify(v));
