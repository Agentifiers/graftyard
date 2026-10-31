// B0-01 and B0-02 on the M0-WP2 Raft prototype. Load comes from parallel /bench calls into graft-raft-edge, and commit
// latency is read on the leader, from a command's admission to its entry's quorum ack (`stats`), so the client's
// distance to the edge does not count.
//
//   node run/raft-bench.ts b0-01 [--seconds 60]     # 100 commands/s of 1 KB on a fresh group led from site A (enam)
//   node run/raft-bench.ts b0-02 [--seconds 30]     # open throttle, one entry in flight

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { raft } from "./raft.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  seconds: { type: "string" }, placement: { type: "string", default: "tri" }, pipe: { type: "string", default: "1" }, rate: { type: "string" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const row = positionals[0];
if (row !== "b0-01" && row !== "b0-02") throw new Error("usage: node run/raft-bench.ts b0-01|b0-02");
const ROOT = new URL("../", import.meta.url).pathname;
// `--pipe N` (N > 1) names the group "plN-...", which the prototype runs with up to N entries in flight (outside the design).
const pipe = Number(opt.pipe);
const prefix = `${pipe > 1 ? `pl${pipe}-` : ""}${opt.placement === "na3" ? "na3-" : ""}`;
const group = `${prefix}${row}-${opt.run}`;
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };

// Creates the group (its first call bootstraps it with A leading) and clears the leader's counters.
await raft("/op", { group, op: { kind: "put", key: "warm", value: "1" } });
await raft("/stats", { group, reset: true });

const seconds = Number(opt.seconds ?? (row === "b0-01" ? 60 : 30));
const CHUNK_S = 15; // one /bench call runs about this long, well under the per-invocation subrequest limit
// `--rate N` runs B0-02 open-loop at N commands/s (the gate's p99 at 300/s) instead of at saturation.
const rate = opt.rate ? Number(opt.rate) : row === "b0-01" ? 100 : 0;
// One edge invocation runs at most 6 member calls at once and queues the rest (the edge's /fanout), so each caller
// keeps 5 workers: more would queue commands in the edge and send them to the leader in bursts.
const WORKERS = 5;
const callers = rate ? Math.ceil(rate / 5) : 16;
type Bench = { colo: string; n: number; failed: number; wallMs: number; lat: number[] };
const client: number[] = []; let failed = 0, done = 0, edgeColo = "";
const t0 = Date.now();
while (Date.now() - t0 < seconds * 1000) {
  // b0-01: 20 callers x 5 workers x 1 op/s = 100 commands/s. b0-02 at saturation: 16 callers x 5 workers, unthrottled.
  const perWorker = rate / callers / WORKERS;
  const body = rate ? { group, n: Math.round(WORKERS * perWorker * CHUNK_S), concurrency: WORKERS, bytes: 1024, ratePerWorker: perWorker }
    : { group, n: 150, concurrency: WORKERS, bytes: 1024 };
  const rs = await Promise.all(Array.from({ length: callers }, () => raft<Bench>("/bench", body).catch((e) => ({ error: String(e) }))));
  for (const r of rs) {
    if ("error" in r) { console.error(r.error); continue; }
    client.push(...r.lat); failed += r.failed; done += r.n - r.failed; edgeColo = r.colo;
  }
}
const wallS = (Date.now() - t0) / 1000;
const followerSplit = await raft<{ role: string; site: string; confirmSplit: [number, number][] }[]>("/stats", { group, reset: false });
const stats0 = await raft<{ role: string; timeline: number[][] }[]>("/stats", { group, reset: false });
// [index, created, sent to first follower, acked by it, sent to second, acked by it, committed]
const tl = stats0.find((x) => x.role === "leader")?.timeline.filter((t) => t[6]) ?? [];
const timelineMs = {
  createdToSent: { p50: pct(tl.map((t) => t[2] - t[1]), 0.5), p99: pct(tl.map((t) => t[2] - t[1]), 0.99) },
  sentToAck: { p50: pct(tl.map((t) => t[3] - t[2]), 0.5), p99: pct(tl.map((t) => t[3] - t[2]), 0.99) },
  ackToCommit: { p50: pct(tl.map((t) => t[6] - Math.min(t[3] || Infinity, t[5] || Infinity)), 0.5), p99: pct(tl.map((t) => t[6] - Math.min(t[3] || Infinity, t[5] || Infinity)), 0.99) },
  n: tl.length,
};
const stats = await raft<{ site: string; role: string; n: number; entries: number; lat: number[]; entryMs: number[]; appendMs: Record<string, number[]>; serveMs: Record<string, number[]>; bytesOut: number[]; maxPending: number }[]>("/stats", { group, reset: true });
const leader = stats.find((s) => s.role === "leader")!;
const status = await raft("/status", { group });
const summary = {
  row, run: opt.run, placement: opt.placement, pipe, maxInFlight: leader.maxPending, group, at: new Date().toISOString(), seconds: wallS, edgeColo, leader: leader.site,
  commands: done, failed, throughputPerS: Math.round(done / wallS), entries: leader.entries,
  commandsPerEntry: Math.round((leader.n / leader.entries) * 10) / 10,
  commitMs: { p50: pct(leader.lat, 0.5), p99: pct(leader.lat, 0.99), max: pct(leader.lat, 1), n: leader.n },
  // One replication round per entry: the leader's own write confirmed, then the nearest follower's ack.
  roundMs: { p50: pct(leader.entryMs, 0.5), p99: pct(leader.entryMs, 0.99) },
  appendMs: Object.fromEntries(Object.entries(leader.appendMs).map(([k, v]) => [k, { p50: pct(v, 0.5), p90: pct(v, 0.9), p99: pct(v, 0.99), n: v.length }])),
  serveMs: Object.fromEntries(Object.entries(leader.serveMs).map(([k, v]) => [k, { p50: pct(v, 0.5), p99: pct(v, 0.99) }])),
  // Per append, the leader's call time minus the follower's own time inside `append`: the wire plus any gate on either
  // side. The two arrays are pushed together for each acked append, so index i is one call.
  wireMs: Object.fromEntries(Object.entries(leader.appendMs).map(([k, v]) => {
    const w = v.map((x, i) => x - (leader.serveMs[k]?.[i] ?? 0));
    return [k, { p50: pct(w, 0.5), p90: pct(w, 0.9), p99: pct(w, 0.99) }];
  })),
  followerCpuConfirmMs: Object.fromEntries(followerSplit.filter((f) => f.role === "follower").map((f) => [f.site, {
    cpuP50: pct(f.confirmSplit.map((x) => x[0]), 0.5), confirmP50: pct(f.confirmSplit.map((x) => x[1]), 0.5),
    confirmP99: pct(f.confirmSplit.map((x) => x[1]), 0.99), n: f.confirmSplit.length }])),
  timelineMs,
  bytesPerAppend: { p50: pct(leader.bytesOut, 0.5), p99: pct(leader.bytesOut, 0.99) },
  clientMs: { p50: pct(client, 0.5), p99: pct(client, 0.99) },
  verdict: row === "b0-01"
    ? (pct(leader.lat, 0.99) <= 150 ? "Pass" : pct(leader.lat, 0.99) > 400 ? "Fail" : "Between pass and fail")
    : (done / wallS >= 300 && pct(leader.lat, 0.99) <= 150 ? "Pass" : done / wallS < 100 ? "Fail" : "Between pass and fail"),
  offeredPerS: rate || "saturation",
  status,
};
mkdirSync(`${ROOT}results`, { recursive: true });
writeFileSync(`${ROOT}results/${row}.json`, JSON.stringify(summary, null, 2));
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/${row}-${prefix}${opt.run}.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify({ ...summary, status: undefined }, null, 2));
