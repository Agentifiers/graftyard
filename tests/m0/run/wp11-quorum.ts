// M0-WP11 (b): do a Durable Object's storage followers span regions? A write is confirmed once 3 of 5 followers hold
// it, so the time `sync()` takes after a write is about the round trip to the third-nearest follower. If that stays
// under the round trip to the nearest other region, the confirming quorum sits inside the object's own region, and a
// confirmed write is not guaranteed to survive the loss of that region (decisions/OPEN-QUESTIONS.md, C1).
//
//   node run/wp11-quorum.ts [--per 10] [--samples 20]

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { per: { type: "string", default: "10" }, samples: { type: "string", default: "20" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const HINTS = ["enam", "wnam", "weur", "eeur", "apac", "oc"];
const median = (xs: number[]) => { const s = [...xs].sort((a, b) => a - b); const m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; };

type Probe = { colo: string; syncMs: number[]; gatedFetchMs: number[]; plainFetchMs: number[] } | { error: string };
const rows: { hint: string; name: string; colo: string; syncMedianMs: number; gateMedianMs: number; plainMedianMs: number; syncMaxMs: number }[] = [];
await Promise.all(HINTS.map(async (hint) => {
  for (let i = 0; i < Number(opt.per); i += 5) {
    const names = Array.from({ length: Math.min(5, Number(opt.per) - i) }, (_, k) => `wp11-${opt.run}-${hint}-${i + k}`);
    const res = await fetch("https://graft-m0-tools.<workers-subdomain>.workers.dev/quorum", { method: "POST",
      headers: { authorization: `Bearer ${TOKEN}` }, body: JSON.stringify({ hint, names, samples: Number(opt.samples) }) });
    const out = await res.json() as Probe[];
    out.forEach((p, k) => {
      if ("error" in p) { console.error(hint, p.error); return; }
      const sync = p.syncMs.slice(1); // the first write also creates the object
      rows.push({ hint, name: names[k], colo: p.colo, syncMedianMs: median(sync), syncMaxMs: Math.max(...sync),
        gateMedianMs: median(p.gatedFetchMs.slice(1).map((g, j) => g - p.plainFetchMs[j + 1])), plainMedianMs: median(p.plainFetchMs) });
    });
  }
  console.error(`${hint} done`);
}));

// The nearest other region, from M0-WP1's member round trips (results/b0-12.json): enam to weur is the shortest
// inter-continent pair at about 70 ms. Within a hint region, B0-12 saw pairs from 1 ms to over 30 ms.
const byHint = Object.fromEntries(HINTS.map((h) => {
  const rs = rows.filter((r) => r.hint === h);
  const s = rs.map((r) => r.syncMedianMs);
  return [h, { objects: rs.length, colos: [...new Set(rs.map((r) => r.colo))], syncMedianMs: median(s), syncMinMs: Math.min(...s), syncMaxMs: Math.max(...s),
    gateMedianMs: median(rs.map((r) => r.gateMedianMs)) }];
}));
const summary = { at: new Date().toISOString(), run: opt.run, samplesPerObject: Number(opt.samples), byHint,
  slowestObjectSyncMedianMs: Math.max(...rows.map((r) => r.syncMedianMs)) };
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(`${ROOT}results/raw/wp11-quorum-${opt.run}.json`, JSON.stringify(rows, null, 1));
writeFileSync(`${ROOT}results/wp11-quorum.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
