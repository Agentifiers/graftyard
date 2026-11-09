// LF-12's commit latency, explained from the members' own timing (am1-fix). On lf1-0 (`FEED_REPO`, `FEED_REPO_ID`):
//   base   25 s of task PATCHes at 2/s with no polls held
//   held   the same with N polls (default 1,000) held on each follower, straight to the members
//   learn  the same with N polls spread over the committed config's learners
// For each phase: client latency, then from every voter's `/stats` (reset before the phase) the leader's `commitMs`,
// `roundMs`, `confirmMs`, and `appendMs` per follower, each follower's `applySteps` (synchronous ms per input that
// applied entries), its 100 ms ticker lag, and its feed counts. Rows go to tests/am1/raw/lf12why.jsonl.
//   node tests/am1/hosted/lf12why.mjs <base|held|learn> [n]
import { api, ghs, log, member, row, sleep, statuses } from "./lib.mjs";

const [which, nArg] = process.argv.slice(2);
const N = Number(nArg ?? 1000);
const OUT = new URL("../raw/lf12why.jsonl", import.meta.url).pathname;
const R = `m3org/${process.env.FEED_REPO ?? "lf1-0"}`;
const CHAIN = `repo:${process.env.FEED_REPO_ID ?? 1048577}`;
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const sum = (xs) => ({ n: xs.length, p50: pct(xs, 0.5), p90: pct(xs, 0.9), p99: pct(xs, 0.99), max: xs.length ? Math.max(...xs) : null });

async function stats(reset, chain = CHAIN) {
  return Promise.all([0, 1, 2].map(async (s) => {
    try { return { site: s, ...(await member(chain, s, `/stats${reset ? "?reset=1" : ""}`, undefined, 15_000)).v }; } catch (e) { return { site: s, error: String(e).slice(0, 80) }; }
  }));
}
// The owner's Authz chain (m3org), which every commit's deps read and the feed's owner refresh reads.
const OWNER = `authz:${process.env.OWNER_ID ?? 7100001}`;

function digest(st) {
  return st.map((s) => {
    const lag = (s.trace ?? []).filter((t) => t[1] === 8).map((t) => t[5]);
    return {
      site: s.site, leader: s.role, entries: s.entries, commands: s.commands,
      commitMs: sum(s.commitMs ?? []), roundMs: sum(s.roundMs ?? []), confirmMs: sum(s.confirmMs ?? []),
      appendMs: Object.fromEntries(Object.entries(s.appendMs ?? {}).map(([k, v]) => [k, sum(v)])),
      applyStepMs: sum((s.applySteps ?? []).map((a) => a[0])), applyMs: sum(s.applyMs ?? []), tickLagMs: sum(lag),
      inbound: s.inbound, failedCalls: s.failedCalls,
    };
  });
}

async function commits(app, id, secs, rate = 2) {
  const ms = [], st = {};
  const end = Date.now() + secs * 1000;
  let k = 0;
  const inflight = [];
  while (Date.now() < end) {
    inflight.push(api("PATCH", `/api/graftyard/v1/repos/${R}/tasks/${id}`, app, { summary: `why ${k++}` }).then((r) => { ms.push(r.ms); st[r.status] = (st[r.status] ?? 0) + 1; }));
    await sleep(1000 / rate);
  }
  await Promise.all(inflight);
  return { status: st, ...sum(ms) };
}

async function direct(site, k, wait, name) {
  const t0 = Date.now();
  try {
    const r = await member(CHAIN, site, "/feed", { cursor: null, wait_s: wait, paths: [`held/${k % 50}/**`], learner_answers: false, token: `w${k}`, ip: `wip${k}` }, (wait + 10) * 1000, name);
    return { admit: r.v?.admit ?? `status ${r.status}`, ms: Date.now() - t0 };
  } catch (e) { return { admit: "error", ms: Date.now() - t0 }; }
}

const app = (await ghs({})).body.token;
let task;
for (let i = 0; i < 5; i++) {
  task = await api("POST", `/api/graftyard/v1/repos/${R}/tasks`, app, { kind: "external", session: `why-${which}-${Date.now().toString(36)}`, summary: "lf12why" });
  if (task.status === 201) break;
  await sleep(3000);
}
// Warm the group (a leader, its deps read) before the timed phase.
await commits(app, task.body.id, 4, 2);
const st0 = await statuses(CHAIN);
const voters = st0.map((s) => [s.site, s.role]);
await stats(true);
await stats(true, OWNER);
let polls = Promise.resolve([]);
let targets = [];
if (which === "held") {
  targets = st0.filter((s) => s.role === "follower").map((s) => ({ site: s.site }));
} else if (which === "learn") {
  const cfg = st0.find((s) => s.config)?.config?.config?.normal;
  targets = (cfg?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name }));
}
if (targets.length) {
  const n = which === "held" ? N * targets.length : N;
  polls = Promise.all(Array.from({ length: n }, (_, k) => sleep((k % 1000) * 3).then(() => {
    const t = targets[k % targets.length];
    return direct(t.site, 200_000 + k, 25, t.name);
  })));
  await sleep(5_000);
}
const c = await commits(app, task.body.id, 25);
const st = await stats(false);
const p = await polls;
const ownerReads = (await stats(false, OWNER)).map((s) => [s.site, s.role, s.inbound?.["/read"] ?? 0]);
const admits = p.reduce((m, r) => ((m[r.admit] = (m[r.admit] ?? 0) + 1), m), {});
const feeds = (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, feed: s.feed }));
const v = { phase: which, n: N, targets: targets.length, voters, client: c, admits, ownerReads, members: digest(st), feeds };
row(OUT, v);
log(JSON.stringify({ phase: which, client: c, admits, ownerReads, members: v.members.map((m) => ({ site: m.site, leader: m.leader, commit: m.commitMs, round: m.roundMs, confirm: m.confirmMs, append: m.appendMs, applyStep: m.applyStepMs, tickLag: m.tickLagMs })) }));
