// B0-26 on the M0-WP2 Raft prototype: a reset storm. Groups take 100 writes/s in total while every member of every
// group, on all three sites, is reset at a random moment inside one 60 s window. Half the load's edge invocations label
// their `suspect` reports with a second colo, because every invocation from this runner lands in one colo. The row
// passes if every group completes a write it started after its own last reset within 12 s of the storm's last reset.
//
//   node run/raft-storm.ts [--groups 4] [--seconds 100] [--window 60]   # --window 1: every member reset at once

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { createLedFromA, raft } from "./raft.ts";

const { values: opt } = parseArgs({ options: { groups: { type: "string", default: "4" }, seconds: { type: "string", default: "100" }, window: { type: "string", default: "60" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const groups = Array.from({ length: Number(opt.groups) }, (_, i) => `b0-26-${opt.run}-g${i}`);
const seconds = Number(opt.seconds);

await raft("/faults", { plan: { rules: [] } });
for (const group of groups) await createLedFromA(group);

type Bench = { colo: string; n: number; failed: number; ops: [number, number, string, boolean][]; errors: Record<string, number> };
// Per group: 5 invocations x 5 workers x 1 op/s = 25 writes/s, so 100/s over 4 groups. One invocation runs at most 6
// member calls at once (M0-WP2, B0-03), so each keeps 5 workers.
const perGroup = 100 / groups.length / 5 / 5;
const load = Promise.all(groups.map((group) => Promise.all(Array.from({ length: 5 }, (_, i) =>
  raft<Bench>("/bench", { group, n: Math.round(5 * perGroup * seconds), concurrency: 5, ratePerWorker: perGroup, bytes: 1024, ops: true,
    colo: i % 2 ? "B0-26-second-colo" : undefined }, (seconds + 60) * 1000)))));

// The storm: from 15 s in, every member of every group is reset at a random time inside 60 s.
const stormStart = Date.now() + 15_000;
const resets = groups.flatMap((group) => ["a", "b", "c"].map((site) => ({ group, site, at: stormStart + Math.random() * Number(opt.window) * 1000 })))
  .sort((x, y) => x.at - y.at);
// Resets are sent without waiting for each other, so a 1 s window resets every member inside about a second.
await Promise.all(resets.map(async (r) => {
  await sleep(Math.max(0, r.at - Date.now()));
  r.at = Date.now();
  await raft("/reset", { group: r.group, sites: [r.site] }).catch(() => {});
}));
const lastReset = Math.max(...resets.map((r) => r.at));
const results = await load;

const perGroupResult = groups.map((group, gi) => {
  const ops = results[gi].flatMap((r) => r.ops).sort((x, y) => x[1] - y[1]);
  const groupLast = Math.max(...resets.filter((r) => r.group === group).map((r) => r.at));
  // The first write started after this group's last reset that completed: the group committed a new entry.
  const first = ops.find((o) => o[3] && o[0] >= groupLast);
  let gap = 0, prev = stormStart;
  for (const o of ops.filter((o) => o[3] && o[1] >= stormStart)) { gap = Math.max(gap, o[1] - prev); prev = o[1]; }
  return { group, ops: ops.length, failed: ops.filter((o) => !o[3]).length,
    firstCommitAfterOwnLastResetMs: first ? first[1] - groupLast : null,
    firstCommitAfterStormEndMs: first ? Math.max(0, first[1] - lastReset) : null,
    longestGapDuringStormMs: gap, errors: Object.assign({}, ...results[gi].map((r) => r.errors)) };
});
const status = await Promise.all(groups.map((group) => raft<{ site: string; role: string; term: number }[]>("/status", { group })));
const worst = Math.max(...perGroupResult.map((g) => g.firstCommitAfterStormEndMs ?? Infinity));
const summary = {
  row: "B0-26", run: opt.run, windowS: Number(opt.window), at: new Date().toISOString(), groups: groups.length, stormMs: lastReset - stormStart,
  resets: resets.map((r) => ({ group: r.group.slice(-2), site: r.site, atMs: r.at - stormStart })),
  perGroup: perGroupResult,
  leaders: status.map((s) => s.filter((m) => m.role === "leader").map((m) => `${m.site}@${m.term}`).join(",")),
  worstMs: worst,
  verdict: worst <= 12_000 ? "Pass" : "Fail",
};
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/b0-26-${opt.run}.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify({ ...summary, resets: undefined }, null, 2));
