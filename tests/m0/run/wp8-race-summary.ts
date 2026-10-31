// Summarizes G6 race files: node run/wp8-race-summary.ts g6-on-r1 g6-nocg-r1 ... (reads results/raw/wp8-<group>.json)
import { readFileSync } from "node:fs";

const ROOT = new URL("../", import.meta.url).pathname;
const rows: Record<string, unknown>[] = [];
for (const g of process.argv.slice(2)) {
  const { trials, noreflog } = JSON.parse(readFileSync(`${ROOT}results/raw/wp8-${g}.json`, "utf8")) as { trials: any[]; noreflog: boolean };
  // Without `ref_log` roots, `ref_log` reaching a missing object is the expected cost of that variant, not a loss.
  if (noreflog) for (const t of trials) t.lost.reflogMissing = [];
  // `lost` lists every missing object reachable from refs at the end of a trial, so one loss repeats in later trials.
  // Each trial is charged only with objects missing for the first time, and passes unless it lost one or broke a
  // per-trial check (the `why` rows that are not the repeated "reach N missing" ones).
  const seen = new Set<string>();
  for (const t of trials) {
    const all = [...t.lost.refsMissing, ...(t.lost.reflogMissing ?? [])];
    t.newLost = [...new Set(all.filter((o: string) => !seen.has(o)))];
    for (const o of all) seen.add(o);
    t.own = t.why.filter((w: string) => !/reach(es)? \d+ missing/.test(w));
    t.ok = t.newLost.length === 0 && t.own.length === 0;
  }
  const count = (f: (t: any) => boolean) => trials.filter(f).length;
  const orders: Record<string, number> = {};
  for (const t of trials) orders[t.order] = (orders[t.order] ?? 0) + 1;
  const sum = (f: (t: any) => number) => trials.reduce((n, t) => n + f(t), 0);
  // A write "interleaved" with GC when it committed or was decided between GC's first root read and its commit.
  const inWindow = count((t) => (t.push.ok && t.push.entry > t.gc.readAt?.[0] && t.push.entry < (t.gc.entry || Infinity))
    || (t.restore.ok && t.restore.entry > t.gc.readAt?.[0] && t.restore.entry < (t.gc.entry || Infinity)));
  rows.push({
    group: g, trials: trials.length, pass: count((t) => t.ok), fail: count((t) => !t.ok), lostObjects: seen.size,
    gcCommitted: count((t) => t.gc.committed), gcAttempts: sum((t) => t.gc.attempts ?? 0), gcCheapReruns: sum((t) => t.gc.cheapReruns ?? 0),
    gcFullReruns: sum((t) => t.gc.fullReruns ?? 0), gcRerunTrials: count((t) => (t.gc.attempts ?? 0) > 1), gcOrphanPacks: sum((t) => t.gc.orphanPacks ?? 0),
    savedByRerun: sum((t) => t.gc.savedByRerun ?? 0), gcMsMedian: trials.map((t) => t.gc.ms).sort((a, b) => a - b)[Math.floor(trials.length / 2)],
    pushOk: count((t) => t.push.ok), pushRefusedMissing: count((t) => !t.push.ok && /missing/.test(t.push.refusals.at(-1) ?? "")),
    pushGuardRefusals: sum((t) => t.push.refusals.filter((r: string) => /compacted since/.test(r)).length),
    restoreOk: count((t) => t.restore.ok), restoreRefusedMissing: count((t) => !t.restore.ok && /missing/.test(t.restore.refusals.at(-1) ?? "")),
    restoreGuardRefusals: sum((t) => t.restore.refusals.filter((r: string) => /compacted since/.test(r)).length),
    unsettled: count((t) => t.push.unsettled || t.restore.unsettled),
    // A refusal for missing objects is wrong if the object was still in a live pack afterwards.
    falseRefusals: count((t) => (!t.push.ok && /missing/.test(t.push.refusals.at(-1) ?? "") && t.lost.present?.[Object.keys(t.lost.present ?? {})[0]])
      || (!t.restore.ok && /missing/.test(t.restore.refusals.at(-1) ?? "") && t.lost.present?.[Object.keys(t.lost.present ?? {})[1]])),
    gcMaxAttempts: Math.max(...trials.map((t) => t.gc.attempts ?? 0)), gcMsMax: Math.max(...trials.map((t) => t.gc.ms ?? 0)),
    retriedWrites: count((t) => t.push.tries > 1 || t.restore.tries > 1), writeInGcWindow: inWindow, orders,
    failures: trials.filter((t) => !t.ok).map((t) => `${t.n} ${t.order}: lost ${t.newLost.length} new; ${t.own.join("; ")}`),
  });
}
console.log(JSON.stringify(rows, null, 1));
