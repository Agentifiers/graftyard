// m0-adaptive-append's hosted check of B0-02's r300-3 shape (`tests/m0/results/carried-9.md`): a group whose home
// and leader are weur (site 1) takes `--rate` commands/s, and after `--stop-at` seconds enam's member (site 0) is
// severed for the rest of the run, so only apac (site 2) can make the quorum. A poller reads every member's `/status`
// once a second. Writes the poll rows, the load's client latencies, and the members' `/stats` to
// tests/m1/wp2/raw/<run>.far.json. The group is made with a weur home rather than moved there with `/transfer`, because a
// moved leader transfers back to its home within one STEP_DOWN window.
//
//   EDGE_URL=... node tests/m0carried/far9.ts --run m0c9-far1 [--rate 300] [--seconds 120] [--stop-at 60]

import { readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  run: { type: "string" }, rate: { type: "string", default: "300" }, seconds: { type: "string", default: "120" },
  "stop-at": { type: "string", default: "60" }, bytes: { type: "string", default: "1024" },
} });
if (!opt.run) throw new Error("usage: node tests/m0carried/far9.ts --run NAME");
const ROOT = new URL("../m1/wp2/", import.meta.url).pathname;
const URL_ = process.env.EDGE_URL!;
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const HOME = 1, NEAR = 0;
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function edge<T = any>(path: string, body?: unknown, ms = 120_000): Promise<T> {
  const res = await fetch(URL_ + path, {
    method: body === undefined ? "GET" : "POST", body: body === undefined ? undefined : JSON.stringify(body),
    headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(ms),
  });
  const text = await res.text();
  if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

let h = 0;
for (const c of opt.run) h = (h * 31 + c.charCodeAt(0)) >>> 0;
const chain = `repo:${900_000 + (h % 1000) * 1000}`;
const created = await edge("/create", { chain, home: HOME });
if (!created.warm?.ok) throw new Error(`create ${chain}: ${JSON.stringify(created).slice(0, 400)}`);
log(`created ${chain}: ${created.verdict} in ${created.leaderMs} ms; placed ${JSON.stringify(created.placed)}`);
await edge("/stats", { chain, home: HOME, reset: true, arrival: "mem", entry: "soft" });

const seconds = Number(opt.seconds), rate = Number(opt.rate), stopAt = Number(opt["stop-at"]);
const t0 = Date.now(), end = t0 + seconds * 1000;
const polls: any[] = [], chunks: any[] = [];
let stoppedAt: number | null = null;

const poller = (async () => {
  while (Date.now() < end + 5_000) {
    const at = Date.now();
    const st = await edge<{ site: number; r: any }[]>(`/status?chain=${chain}&home=${HOME}`).catch(() => null);
    if (st) polls.push({ at, m: st.map((x) => x.r ? [x.site, x.r.role, x.r.term, x.r.last, x.r.commit] : [x.site, null]) });
    await sleep(Math.max(0, 1000 - (Date.now() - at)));
  }
})();

const stopper = (async () => {
  await sleep(stopAt * 1000);
  const r = await edge("/member", { chain, home: HOME, site: NEAR, path: "/sever", body: { for_ms: (seconds - stopAt + 30) * 1000 } });
  stoppedAt = Date.now();
  log(`severed site ${NEAR}: ${JSON.stringify(r)}`);
})();

// As run.ts's bench: callers x 5 workers x 1 op/s, each call about 15 s.
const WORKERS = 5, CHUNK_S = 15, callers = Math.ceil(rate / WORKERS), perWorker = rate / callers / WORKERS;
while (Date.now() < end) {
  const at = Date.now();
  const rs = await Promise.all(Array.from({ length: callers }, () =>
    edge<any>("/bench", { chain, home: HOME, n: Math.round(WORKERS * perWorker * CHUNK_S), concurrency: WORKERS, bytes: Number(opt.bytes), ratePerWorker: perWorker }, 300_000)
      .catch((e) => ({ error: String(e) }))));
  const lat = rs.flatMap((r) => r.lat ?? []), failed = rs.reduce((n, r) => n + (r.failed ?? 0), 0);
  chunks.push({ at, ms: Date.now() - at, ok: lat.length, failed, p50: pct(lat, 0.5), p99: pct(lat, 0.99), max: pct(lat, 1) });
  log(`chunk at +${Math.round((at - t0) / 1000)} s: ${lat.length} ok, ${failed} failed, p99 ${pct(lat, 0.99)} ms`);
}
await Promise.all([poller, stopper]);
const stats = await edge("/stats", { chain, home: HOME }).catch((e) => ({ error: String(e) }));

function pct(xs: number[], p: number) { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(s.length * p))] : null; }

// From the polls: apac's lag behind the leader, and the stall: from the sever until the group's commit passes the
// leader's last index at the sever.
const leaderOf = (p: any) => p.m.find((x: any) => x[1] === "leader");
const lagAt = (p: any) => { const l = leaderOf(p), a = p.m.find((x: any) => x[0] === 2); return l && a?.[3] != null ? l[3] - a[3] : null; };
const atStop = stoppedAt === null ? null : [...polls].reverse().find((p) => p.at <= stoppedAt!);
const lastAtStop = atStop ? leaderOf(atStop)?.[3] ?? null : null;
const resumed = lastAtStop === null ? null : polls.find((p) => p.at >= stoppedAt! && (leaderOf(p)?.[4] ?? 0) > lastAtStop);
const s = {
  run: opt.run, chain, rate, seconds, stopAt, t0, stoppedAt,
  leaders: [...new Set(polls.map((p) => `${leaderOf(p)?.[0]}@${leaderOf(p)?.[2]}`))],
  apacBehindAtStop: atStop ? lagAt(atStop) : null,
  apacBehindMaxBeforeStop: Math.max(...polls.filter((p) => stoppedAt === null || p.at < stoppedAt).map(lagAt).filter((x) => x !== null) as number[]),
  stallMs: resumed && stoppedAt ? resumed.at - stoppedAt : null,
  chunks, polls,
};
writeFileSync(`${ROOT}raw/${opt.run}.far.json`, JSON.stringify(s));
writeFileSync(`${ROOT}raw/${opt.run}.stats.json`, JSON.stringify(stats));
log(JSON.stringify({ ...s, polls: undefined, chunks: chunks.map((c) => [Math.round((c.at - t0) / 1000), c.ok, c.failed, c.p99, c.max]) }));
