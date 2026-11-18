// B0-12, member placement (build/BUILD-PLAN.md, M0-WP1). Creates `groups` three-voter groups on `Tri` (one member per
// site, each with its site's hint), has every member take the median of 20 binding round trips to each other voter,
// and reports the share of voter pairs at MIN_PAIR_RTT or more whose members sit in their hinted regions.
// It also measures intra-region pairs (two members with the same hint on one site), which the plan asks for beside
// B0-12. Usage: node run/b0-12.ts --groups 1000 --intra 50 [--run id] [--batch 20]

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { placementNew, MIN_PAIR_RTT_MS } from "../src/placement.ts";
import { SITE_KEYS, SITES, type MemberInfo, type MemberRef, type Region, type RttRow } from "../src/sites.ts";

const { values: opt } = parseArgs({ options: {
  groups: { type: "string", default: "1000" }, intra: { type: "string", default: "50" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 15) },
  batch: { type: "string", default: "20" },
  edge: { type: "string", default: `https://graft-m0-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev` },
} });
const GROUPS = Number(opt.groups), INTRA = Number(opt.intra), BATCH = Number(opt.batch), RUN = opt.run!;
const TOKEN = readFileSync(new URL("../.m0-token", import.meta.url), "utf8").trim();

type Settled<T> = { ok: true; value: T } | { ok: false; error: string };

async function edge<T>(path: string, body: unknown): Promise<{ colo: string; results: Settled<T>[] }> {
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(opt.edge + path, { method: "POST", body: JSON.stringify(body),
      headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" } });
    if (res.ok) return res.json() as Promise<{ colo: string; results: Settled<T>[] }>;
    if (attempt >= 3) throw new Error(`${path}: HTTP ${res.status} ${await res.text()}`);
    await new Promise((r) => setTimeout(r, 2000 * attempt));
  }
}

async function inBatches<J, T>(label: string, jobs: J[], path: string, wrap: (b: J[]) => unknown, parallel = 4): Promise<Settled<T>[]> {
  const batches: J[][] = [];
  for (let i = 0; i < jobs.length; i += BATCH) batches.push(jobs.slice(i, i + BATCH));
  const out: Settled<T>[][] = new Array(batches.length);
  let next = 0, done = 0;
  await Promise.all(Array.from({ length: parallel }, async () => {
    while (next < batches.length) {
      const i = next++;
      out[i] = (await edge<T>(path, wrap(batches[i]))).results;
      if (++done % 10 === 0 || done === batches.length) console.error(`${label}: ${done}/${batches.length} batches`);
    }
  }));
  return out.flat();
}

// Colo to continent, from the Cloudflare status page's data-center components ("Amsterdam, Netherlands - (AMS)").
async function coloContinents(): Promise<Map<string, string>> {
  const { components } = await (await fetch("https://www.cloudflarestatus.com/api/v2/components.json")).json() as
    { components: { id: string; name: string; group: boolean; group_id: string | null }[] };
  const groups = new Map(components.filter((c) => c.group).map((c) => [c.id, c.name]));
  const map = new Map<string, string>();
  for (const c of components) {
    const code = /\(([A-Z]{3})\)\s*$/.exec(c.name)?.[1];
    if (code && c.group_id) map.set(code, groups.get(c.group_id)!);
  }
  return map;
}
// A hint is consistent when the member's colo is on the hint's continent. This cannot tell enam from wnam, which
// `Tri` does not need, since its hints are on three continents [inference].
const HINT_CONTINENT: Record<Region, string[]> = {
  enam: ["North America"], wnam: ["North America"], weur: ["Europe"], eeur: ["Europe"],
  apac: ["Asia"], oc: ["Oceania"],
};

const groups: MemberRef[][] = Array.from({ length: GROUPS }, (_, g) =>
  SITE_KEYS.map((k, slot) => ({ site: k, name: `b012-${RUN}-g${g}#1#f${slot}`, hint: SITES[k].hint })));
const intra: [MemberRef, MemberRef][] = SITE_KEYS.flatMap((k) => Array.from({ length: INTRA }, (_, i) =>
  [0, 1].map((slot) => ({ site: k, name: `b012-${RUN}-intra-${k}${i}#1#f${slot}`, hint: SITES[k].hint })) as [MemberRef, MemberRef]));
const allMembers = [...groups.flat(), ...intra.flat()];

console.error(`run ${RUN}: ${GROUPS} groups, ${INTRA} intra pairs per site, ${allMembers.length} members`);
const continents = await coloContinents();
const created = await inBatches<MemberRef, MemberInfo>("create", allMembers, "/create", (b) => ({ members: b }));
const info = new Map<string, MemberInfo>();
created.forEach((r) => { if (r.ok) info.set(r.value.name, r.value); });

const jobs = [
  ...groups.flatMap((g) => g.map((m) => ({ member: m, peers: g.filter((p) => p !== m) }))),
  ...intra.map(([x, y]) => ({ member: x, peers: [y] })),
];
const measured = await inBatches<{ member: MemberRef; peers: MemberRef[] }, RttRow[]>("measure", jobs, "/measure", (b) => ({ jobs: b }));
const rows: RttRow[] = measured.flatMap((r) => (r.ok ? r.value : []));
const failures = [...created, ...measured].filter((r) => !r.ok).map((r) => (r as { error: string }).error);

const consistent = (name: string) => {
  const m = info.get(name);
  return !!m && HINT_CONTINENT[m.hint].includes(continents.get(m.colo) ?? "?");
};
const intraNames = new Set(intra.flat().map((m) => m.name));
const voterRows = rows.filter((r) => !intraNames.has(r.from));
const intraRows = rows.filter((r) => intraNames.has(r.from));
const passing = voterRows.filter((r) => r.medianMs >= MIN_PAIR_RTT_MS && consistent(r.from) && consistent(r.peer));
const expectedPairs = GROUPS * 6;
const share = passing.length / expectedPairs;

const placementRefusals = groups.map((g) => placementNew(
  g.map((m) => ({ site: m.site, name: m.name, region: m.hint, account: "one", kind: "Full" as const })),
  voterRows.filter((r) => g.some((m) => m.name === r.from)), false)).filter((r) => r.length > 0);

const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))]; };
const byPair = new Map<string, number[]>();
for (const r of voterRows) {
  const key = [info.get(r.from)?.site, r.peerSite].sort().join("-");
  byPair.set(key, [...(byPair.get(key) ?? []), r.medianMs]);
}
const colos = new Map<string, number>();
for (const m of info.values()) colos.set(`${m.site}:${m.colo}`, (colos.get(`${m.site}:${m.colo}`) ?? 0) + 1);

const summary = {
  run: RUN, at: new Date().toISOString(), groups: GROUPS, members: allMembers.length,
  voterPairs: { expected: expectedPairs, measured: voterRows.length, passing: passing.length, share },
  verdict: share >= 0.95 ? "Pass" : share < 0.80 ? "Fail" : "Between pass and fail (not done, raise it)",
  inconsistentMembers: [...info.values()].filter((m) => !consistent(m.name)).map((m) => `${m.name} ${m.hint} ${m.colo}`).slice(0, 20),
  pairMedianMs: Object.fromEntries([...byPair].map(([k, v]) => [k, { p0: pct(v, 0), p50: pct(v, 0.5), p99: pct(v, 0.99), n: v.length }])),
  intraRegionMs: Object.fromEntries(SITE_KEYS.map((k) => {
    const v = intraRows.filter((r) => r.peerSite === k).map((r) => r.medianMs);
    return [SITES[k].hint, v.length ? { p0: pct(v, 0), p50: pct(v, 0.5), p99: pct(v, 0.99), max: Math.max(...v), n: v.length } : null];
  })),
  placementNewRefusedGroups: placementRefusals.length,
  memberColos: Object.fromEntries([...colos].sort((a, b) => b[1] - a[1])),
  failures: { count: failures.length, sample: failures.slice(0, 5) },
};

mkdirSync(new URL("../results/raw/", import.meta.url), { recursive: true });
writeFileSync(new URL(`../results/raw/b0-12-${RUN}.json`, import.meta.url), JSON.stringify({ summary, info: [...info.values()], rows }, null, 1));
console.log(JSON.stringify(summary, null, 2));
