// Container placement under the `durable_object` scheduling policy (decisions/OPEN-QUESTIONS.md, C19). Wrangler rejects
// `constraints.regions` for that policy, so a git cache shard cannot pin its container to its site's region in config.
// This starts `--per` containers from Durable Objects created with each site's hint, reads the region the runtime
// reports inside each container, and destroys it. Run it from more than one client location if the result is close.
//
//   node run/container-placement.ts [--per 30] [--hints enam,weur,apac] [--no-summary]
// B0-19 runs it with `--per 34 --no-summary`, so its 102 starts leave results/container-placement.json alone.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ allowNegative: true, options: { summary: { type: "boolean", default: true }, per: { type: "string", default: "30" }, hints: { type: "string", default: "enam,weur,apac" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const TOKEN = readFileSync(new URL("../.m0-token", import.meta.url), "utf8").trim();
const URL_ = `https://graft-m0-container.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const PER = Number(opt.per), HINTS = opt.hints!.split(",");

type Landing = { doColo: string; started: boolean; startMs: number; region?: string; location?: string; error?: string };

async function land(hint: string, i: number): Promise<Landing & { hint: string }> {
  for (let attempt = 1; ; attempt++) {
    try {
      const res = await fetch(`${URL_}/?hint=${hint}&name=place-${opt.run}-${hint}-${i}`, { headers: { authorization: `Bearer ${TOKEN}` } });
      if (res.ok) return { hint, ...(await res.json() as Landing) };
      if (attempt >= 3) return { hint, doColo: "?", started: false, startMs: 0, error: `HTTP ${res.status}` };
    } catch (e) { if (attempt >= 3) return { hint, doColo: "?", started: false, startMs: 0, error: String(e) }; }
    await new Promise((r) => setTimeout(r, 2000 * attempt));
  }
}

// Five at a time keeps well under the account's concurrent-instance limits; each container is destroyed after one call.
const jobs = HINTS.flatMap((h) => Array.from({ length: PER }, (_, i) => [h, i] as const));
const rows: (Landing & { hint: string })[] = [];
let next = 0;
await Promise.all(Array.from({ length: 5 }, async () => {
  while (next < jobs.length) { const [h, i] = jobs[next++]; rows.push(await land(h, i)); }
}));

const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))]; };
const summary = {
  run: opt.run, at: new Date().toISOString(), per: PER,
  byHint: Object.fromEntries(HINTS.map((h) => {
    const rs = rows.filter((r) => r.hint === h), ok = rs.filter((r) => r.started);
    const count = (k: "region" | "location" | "doColo") => Object.fromEntries(Object.entries(Object.groupBy(ok, (r) => r[k] ?? "?")).map(([v, x]) => [v, x!.length]));
    return [h, { started: ok.length, of: rs.length, inHintRegion: ok.filter((r) => r.region?.toLowerCase() === h).length,
                 containerRegions: count("region"), containerLocations: count("location"), doColos: count("doColo"),
                 startMs: ok.length ? { p50: pct(ok.map((r) => r.startMs), 0.5), p99: pct(ok.map((r) => r.startMs), 0.99) } : null,
                 errors: rs.filter((r) => !r.started).map((r) => r.error).slice(0, 3) }];
  })),
};
mkdirSync(new URL("../results/raw/", import.meta.url), { recursive: true });
writeFileSync(new URL(`../results/raw/container-placement-${opt.run}.json`, import.meta.url), JSON.stringify({ summary, rows }, null, 1));
if (opt.summary) writeFileSync(new URL("../results/container-placement.json", import.meta.url), JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
