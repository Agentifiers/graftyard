// B0-03 on the M0-WP2 Raft prototype: write and read RTO when the leader's site is lost. 100 operations a second
// (80% writes, 20% `fresh` reads) run through one edge, so every client sits in one colo. After 10 s, every member call
// to and from site A, the leader's site, fails (`error`) or hangs (`hang`, blackholed). RTO is the time from the fault to
// the first write and the first read that a new leader completes, and the longest gap with no completed operation.
//
//   node run/raft-rto.ts error|hang [--seconds 40]

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { createLedFromA, raft } from "./raft.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  seconds: { type: "string", default: "40" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const mode = positionals[0];
if (mode !== "error" && mode !== "hang") throw new Error("usage: node run/raft-rto.ts error|hang");
const ROOT = new URL("../", import.meta.url).pathname;
const group = `b0-03-${mode}-${opt.run}`;
const seconds = Number(opt.seconds);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

await raft("/faults", { plan: { rules: [] } });
await sleep(2_500); // every isolate drops a plan left by an earlier run (cached up to 1 s) before the group starts
await createLedFromA(group);
const before = await raft<{ site: string; role: string; colo?: string }[]>("/status", { group });
if (before.find((s) => s.role === "leader")?.site !== "a") throw new Error(`site a must lead before the fault: ${JSON.stringify(before)}`);

type Bench = { colo: string; n: number; failed: number; notes: string[]; ops: [number, number, string, boolean][]; errors: Record<string, number> };
// 20 callers x 5 workers x at most 1 op/s = 100 ops/s. One invocation runs at most 6 member calls at once and queues
// the rest (measured with the edge's /fanout), so a caller with more workers makes its own calls miss CALL_DEADLINE.
const load = Promise.all(Array.from({ length: 20 }, () => raft<Bench>("/bench", { group, n: 5 * seconds, concurrency: 5, ratePerWorker: 1,
  bytes: 1024, readShare: 0.2, ops: true })));
await sleep(7_000);
const hangMs = 120_000;
const rule = (match: string) => ({ target: "call", mode, rate: 1, match, ...(mode === "hang" ? { hangMs } : {}) });
// The fault starts 3 s after it is set, once every isolate has the plan cached, so it begins at one moment for all.
const faultAt = Date.now() + 3_000;
await raft("/faults", { plan: { rules: [rule(">a:"), rule("a>")], from: faultAt } });
const results = await load;
await raft("/faults", { plan: { rules: [] } });
await sleep(2_500); // isolates still holding the fault plan would hang the calls below
const memberLog = (await raft<{ site: string; log?: string[] }[]>("/stats", { group, reset: false })).map((m) => ({ site: m.site, log: m.log }));
const after = await raft<{ site: string; role: string; term: number }[]>("/status", { group });

const ops = results.flatMap((r) => r.ops).sort((x, y) => x[1] - y[1]);
const okAfter = ops.filter((o) => o[3] && o[1] >= faultAt);
// A completion counts as the new leader's only if the operation started after the fault: an older one may have been
// answered by the old leader before the fault took effect in every isolate.
// RTO runs from the fault to the first write and the first read the new leader completes after the longest gap.
let gapEnd = faultAt, gap = 0;
{ let prevT = faultAt; for (const o of okAfter) { if (o[1] - prevT > gap) { gap = o[1] - prevT; gapEnd = o[1]; } prevT = o[1]; } }
const firstNew = (kind: string) => okAfter.find((o) => o[2] === kind && o[1] >= gapEnd);
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const latAfter = okAfter.map((o) => o[1] - o[0]);
const writeRto = firstNew("put") ? firstNew("put")![1] - faultAt : null;
const readRto = firstNew("get") ? firstNew("get")![1] - faultAt : null;
const offered = ops.filter((o) => o[0] < faultAt).length / ((faultAt - Math.min(...ops.map((o) => o[0]))) / 1000);
const summary = {
  row: "B0-03", opsPerSBeforeFault: Math.round(offered), mode, run: opt.run, group, at: new Date().toISOString(), edgeColo: results[0].colo,
  leaderBefore: before.find((s) => s.role === "leader")?.site, leaderAfter: after.find((s) => s.role === "leader")?.site,
  termAfter: after.find((s) => s.role === "leader")?.term,
  ops: ops.length, failed: ops.filter((o) => !o[3]).length, errors: Object.assign({}, ...results.map((r) => r.errors)),
  writeRtoMs: writeRto, readRtoMs: readRto, longestGapMs: gap,
  latencyAfterFaultMs: { p50: pct(latAfter, 0.5), p99: pct(latAfter, 0.99), max: pct(latAfter, 1) },
  // One run gives one RTO sample, so "p99" over a single failover is the sample itself; the gate's p99 needs repeats.
  verdict: writeRto !== null && readRto !== null && Math.max(writeRto, readRto) <= 12_000 ? "Pass (one sample)"
    : writeRto === null || readRto === null || Math.max(writeRto, readRto) > 60_000 ? "Fail" : "Between pass and fail",
};
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/b0-03-${mode}-${opt.run}.json`, JSON.stringify({ summary, ops, notes: results.map((r) => r.notes), memberLog, faultAt }, null, 1));
console.log(JSON.stringify(summary, null, 2));
