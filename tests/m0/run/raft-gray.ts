// B0-06 on the M0-WP2/WP3 Raft prototype: a gray site. Groups led on site A take 100 writes/s in total. Then 30% of
// every member call to and from A fails (`calls`), or separately 30% of A's storage writes fail and reset the member
// (`writes`), for `--hold` seconds. The row passes if every group leads off A within 20 s of the fault and changes
// leader at most twice in the hour; this runner holds the fault for --hold seconds and counts changes in that time.
//
//   node run/raft-gray.ts calls|writes|slow|edge|none [--groups 4] [--hold 120] [--rate 0.3] [--load 100] [--hang 1000]
// `slow` makes `--rate` of A's storage writes confirm `--hang` ms late instead of failing (gray storage, M0-WP4 rerun);
// `none` injects nothing and only counts leader changes and `STEP_DOWN` trips. `--load` is total writes/s.
// `--prefix min3` names the groups `min3-*`, which the member treats as the F37 variant (M0-WP4).
// `edge` fails only the edge's calls to A, so the leader sees nothing wrong with its followers and only the two-colo
// `suspect` rule can move it: followers stop renewing its promise, its lease lapses, and a follower campaigns.

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { createLedFromA, raft, RAFT_URL } from "./raft.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: { groups: { type: "string", default: "4" },
  hold: { type: "string", default: "120" }, rate: { type: "string", default: "0.3" }, load: { type: "string", default: "100" },
  hang: { type: "string", default: "1000" }, prefix: { type: "string", default: "b0-06" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const mode = positionals[0];
if (!["calls", "writes", "slow", "edge", "none"].includes(mode)) throw new Error("usage: node run/raft-gray.ts calls|writes|slow|edge|none");
const ROOT = new URL("../", import.meta.url).pathname;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const groups = Array.from({ length: Number(opt.groups) }, (_, i) => `${opt.prefix}-${mode}-${opt.run}-g${i}`);
const hold = Number(opt.hold) * 1000, rate = Number(opt.rate);

await raft("/faults", { plan: { rules: [] } });
await sleep(2_500);
for (const group of groups) await createLedFromA(group);

type Bench = { ops: [number, number, string, boolean][]; errors: Record<string, number> };
const perWorker = Number(opt.load) / groups.length / 5 / 5;
const seconds = 15 + hold / 1000 + 10;
// Each caller runs /bench calls of at most 120 s back to back: one HTTP call longer than about 300 s times out in the
// runner's fetch (the 10-minute healthy run).
const loadEnd = Date.now() + seconds * 1000;
const load = Promise.all(groups.map((group) => Promise.all(Array.from({ length: 5 }, async (_, i) => {
  const out: Bench[] = [];
  for (let left = loadEnd - Date.now(); left > 1_000; left = loadEnd - Date.now()) {
    const s = Math.min(120, left / 1000);
    out.push(await raft<Bench>("/bench", { group, n: Math.max(1, Math.round(5 * perWorker * s)), concurrency: 5, ratePerWorker: perWorker, bytes: 1024,
      ops: true, colo: i % 2 ? "B0-06-second-colo" : undefined }, (s + 60) * 1000).catch((e) => ({ ops: [], errors: { [String(e).slice(0, 80)]: 1 } })));
  }
  return { ops: out.flatMap((b) => b.ops), errors: Object.assign({}, ...out.map((b) => b.errors)) } as Bench;
}))));

// Leader per group, sampled every second.
const samples: { at: number; leaders: (string | null)[] }[] = [];
let sampling = true;
const sampler = (async () => {
  while (sampling) {
    const st = await Promise.all(groups.map((group) => raft<{ site: string; role: string; term: number }[]>("/status", { group })
      .then((s) => s.find((m) => m.role === "leader")?.site ?? null, () => null)));
    samples.push({ at: Date.now(), leaders: st });
    await sleep(1_000);
  }
})();

await sleep(12_000);
const faultAt = Date.now() + 3_000;
const rules = mode === "calls"
  ? [{ target: "call", mode: "error", rate, match: ">a:" }, { target: "call", mode: "error", rate, match: "a>" }]
  : mode === "edge" ? [{ target: "call", mode: "error", rate, match: "edge>a:" }]
  : mode === "slow" ? [{ target: "write", mode: "slow-reply", rate, match: "a:", hangMs: Number(opt.hang) }]
  : mode === "none" ? []
  : [{ target: "write", rate, match: "a:" }];
await raft("/faults", { plan: { rules, from: faultAt, until: faultAt + hold } });
const results = await load;
await sleep(3_000);
sampling = false;
await sampler;
await raft("/faults", { plan: { rules: [] } });
await sleep(2_500);
const logs = await Promise.all(groups.map((group) => raft<{ site: string; log?: string[] }[]>("/stats", { group, reset: false })
  .then((ms) => ms.map((m) => ({ site: m.site, log: (m.log ?? []).filter((l) => /STEP_DOWN|won term|ready|vote|lost|suspect|storage confirm|follower confirm|hand-off|append to/.test(l))
    .filter((l, i, a) => !/suspect from/.test(l) || !/suspect from/.test(a[i - 1] ?? "")) })))));

// Each member's own count of completed `STEP_DOWN` transfers (in memory; a reset member starts again at 0).
const stepDownCounts = await Promise.all(groups.map((group) => raft<{ stepDowns?: number }[]>("/status", { group })
  .then((ms) => ms.reduce((n, m) => n + (m.stepDowns ?? 0), 0), () => null)));
const perGroup = groups.map((group, gi) => {
  const seq = samples.filter((s) => s.at >= faultAt - 2_000).map((s) => ({ at: s.at, leader: s.leaders[gi] }));
  const movedAt = seq.find((s) => s.at >= faultAt && s.leader && s.leader !== "a")?.at;
  let changes = 0, prev: string | null = "a";
  for (const s of seq) if (s.leader && s.leader !== prev) { changes++; prev = s.leader; }
  const ops = results[gi].flatMap((r) => r.ops);
  const during = ops.filter((o) => o[0] >= faultAt && o[0] < faultAt + hold);
  const lines = logs[gi].flatMap((m) => m.log);
  return { group, movedMs: movedAt ? movedAt - faultAt : null, leaderChanges: changes, finalLeader: prev,
    stepDowns: lines.filter((l) => /STEP_DOWN in term/.test(l)).length, stepDownsByStatus: stepDownCounts[gi], stepDownHeld: lines.filter((l) => /STEP_DOWN held/.test(l)).length,
    slowConfirms: lines.filter((l) => /storage confirm took/.test(l)).length,
    opsDuringFault: during.length, failedDuringFault: during.filter((o) => !o[3]).length, log: logs[gi] };
});
const worst = Math.max(...perGroup.map((g) => g.movedMs ?? Infinity));
const summary = {
  row: "B0-06", mode, run: opt.run, at: new Date().toISOString(), rate, holdS: hold / 1000, groups: groups.length, load: Number(opt.load),
  hangMs: mode === "slow" ? Number(opt.hang) : undefined, url: RAFT_URL,
  stepDowns: perGroup.reduce((n, g) => n + g.stepDowns, 0), slowConfirms: perGroup.reduce((n, g) => n + g.slowConfirms, 0),
  opsDuringFault: perGroup.reduce((n, g) => n + g.opsDuringFault, 0), failedDuringFault: perGroup.reduce((n, g) => n + g.failedDuringFault, 0),
  perGroup, worstMovedMs: worst, maxChanges: Math.max(...perGroup.map((g) => g.leaderChanges)),
  verdict: mode === "none" ? (perGroup.every((g) => g.leaderChanges === 0) ? "Pass" : "Fail")
    : worst <= 20_000 && perGroup.every((g) => g.leaderChanges <= 2) ? "Pass" : "Fail",
};
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/b0-06-${mode}-${opt.run}.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
