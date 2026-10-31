// B0-18 on the M0-WP2/WP3 Raft prototype: learners. N learners (slots 0..N-1, spread over the three sites) each serve
// `--rate` fresh reads a second for `--seconds`, while the group takes 10 writes/s. The row passes if the leader stays
// under 700 requests a second and learner read p99 is at most 150 ms. Reads are generated inside each learner (its
// `selfLoad`), so the learner's own per-object request limit is not exercised here; the leader's load is real.
//
//   node run/raft-learners.ts [--learners 16] [--rate 1000] [--seconds 30]

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { raft } from "./raft.ts";

const { values: opt } = parseArgs({ options: { learners: { type: "string", default: "16" }, rate: { type: "string", default: "1000" },
  seconds: { type: "string", default: "30" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const group = `b0-18-${opt.run}`;
const n = Number(opt.learners), seconds = Number(opt.seconds), rate = Number(opt.rate);
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };

await raft("/faults", { plan: { rules: [] } });
for (let k = 0; k < 64; k++) await raft("/op", { group, op: { kind: "put", key: `k${k}`, value: "0" } }).catch(() => {});
await raft("/stats", { group, reset: true });
const writes = raft("/bench", { group, n: 10 * seconds, concurrency: 5, bytes: 1024, ratePerWorker: 2 });
type L = { slot: number; site: string; colo: string; n: number; failed: number; lat: number[] };
const ls = await Promise.all(Array.from({ length: n }, (_, slot) => raft<L>("/learner", { group, slot, seconds, rate, keys: 64 })
  .catch((e) => ({ slot, error: String(e) }))));
await writes;
const stats = await raft<{ site: string; role: string; reqs: Record<string, number>; reqsSeconds: number }[]>("/stats", { group, reset: false });
const leader = stats.find((s) => s.role === "leader")!;
const leaderReqs = Object.values(leader.reqs).reduce((a, b) => a + b, 0);
const ok = ls.filter((l): l is L => "lat" in l);
const lat = ok.flatMap((l) => l.lat);
const summary = {
  row: "B0-18", run: opt.run, group, at: new Date().toISOString(), learners: n, ratePerLearner: rate, seconds,
  reads: ok.reduce((a, l) => a + l.n, 0), readsPerS: Math.round(ok.reduce((a, l) => a + l.n - l.failed, 0) / seconds),
  failed: ok.reduce((a, l) => a + l.failed, 0), errors: ls.filter((l) => "error" in l),
  learnerReadMs: { p50: pct(lat, 0.5), p99: pct(lat, 0.99), max: pct(lat, 1) },
  perLearner: ok.map((l) => ({ slot: l.slot, site: l.site, colo: l.colo, n: l.n, failed: l.failed, p50: pct(l.lat, 0.5), p99: pct(l.lat, 0.99) })),
  leader: { site: leader.site, reqs: leader.reqs, reqsPerS: Math.round(leaderReqs / leader.reqsSeconds), seconds: leader.reqsSeconds },
  verdict: leaderReqs / leader.reqsSeconds < 700 && pct(lat, 0.99) <= 150 ? "Pass" : "Fail",
};
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/b0-18-${opt.run}.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify({ ...summary, perLearner: undefined }, null, 2));
