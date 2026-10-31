// B0-13, bucket placement (build/BUILD-PLAN.md, M0-WP1). Creates 20 buckets per R2 location hint, reads `location`
// back from the get-bucket API, and finds where each bucket really is by timing reads from a probe Durable Object in
// each region. Pass: the API's `location` agrees with the probe for 100% of buckets.
//
//   node run/b0-13.ts            # create buckets (idempotent), deploy graft-m0-probe, probe, report
//   node run/b0-13.ts --delete   # empty and delete the test buckets and the probe script

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import type { Region } from "../src/sites.ts";

const { values: opt } = parseArgs({ options: { per: { type: "string", default: "20" }, delete: { type: "boolean", default: false } } });
const HINTS: Region[] = ["enam", "wnam", "weur", "eeur", "apac", "oc"];
const PER = Number(opt.per);
const { CLOUDFLARE_API_TOKEN: CF_TOKEN, CLOUDFLARE_ACCOUNT_ID: ACCOUNT } = process.env;
const TOKEN = readFileSync(new URL("../.m0-token", import.meta.url), "utf8").trim();
const SCRIPT = "graft-m0-probe";
const subdomain = "<workers-subdomain>";

async function cf<T>(path: string, init?: RequestInit): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    const res = await fetch(`https://api.cloudflare.com/client/v4/accounts/${ACCOUNT}${path}`, { ...init,
      headers: { authorization: `Bearer ${CF_TOKEN}`, "content-type": "application/json" } });
    const j = await res.json() as { success: boolean; errors: { code: number; message: string }[]; result: T };
    if (j.success) return j.result;
    if (attempt >= 4 || res.status < 500 && res.status !== 429) throw new Error(`${path}: ${JSON.stringify(j.errors)}`);
    await new Promise((r) => setTimeout(r, 2000 * attempt));
  }
}

const buckets = HINTS.flatMap((hint) => Array.from({ length: PER }, (_, i) => ({ hint, name: `graft-m0-b013-${hint}-${i}`, binding: `B_${hint.toUpperCase()}_${i}` })));

if (opt.delete) {
  for (const b of buckets) {
    try { execFileSync("npx", ["wrangler", "r2", "object", "delete", `${b.name}/b0-13-probe`, "--remote"], { stdio: "ignore" }); } catch {}
    try { await cf(`/r2/buckets/${b.name}`, { method: "DELETE" }); } catch (e) { console.error(String(e)); }
  }
  try { await cf(`/workers/scripts/${SCRIPT}`, { method: "DELETE" }); } catch (e) { console.error(String(e)); }
  console.log("deleted");
  process.exit(0);
}

// 1. Buckets, then `location` from the get-bucket API.
const existing = new Set((await cf<{ buckets: { name: string }[] }>("/r2/buckets?per_page=1000")).buckets.map((b) => b.name));
for (const b of buckets) if (!existing.has(b.name)) await cf("/r2/buckets", { method: "POST", body: JSON.stringify({ name: b.name, locationHint: b.hint }) });
const apiLocation = new Map<string, string>();
// The API returns upper case ("ENAM"); hints are lower case.
for (const b of buckets) apiLocation.set(b.name, ((await cf<{ location?: string }>(`/r2/buckets/${b.name}`)).location ?? "none").toLowerCase());
console.error(`buckets ready: ${buckets.length}`);

// 2. The probe script, binding every bucket.
mkdirSync(".gen", { recursive: true });
writeFileSync(".gen/probe.jsonc", JSON.stringify({
  name: SCRIPT, main: "../src/probe.ts", compatibility_date: "2026-10-01", workers_dev: true, preview_urls: false,
  durable_objects: { bindings: [{ name: "PROBE", class_name: "Probe" }] },
  migrations: [{ tag: "v1", new_sqlite_classes: ["Probe"] }],
  r2_buckets: buckets.map((b) => ({ binding: b.binding, bucket_name: b.name })),
}, null, 2));
execFileSync("npx", ["wrangler", "deploy", "-c", ".gen/probe.jsonc"], { stdio: ["ignore", "ignore", "inherit"] });
execFileSync("npx", ["wrangler", "secret", "put", "M0_TOKEN", "-c", ".gen/probe.jsonc"], { input: TOKEN, stdio: ["pipe", "ignore", "inherit"] });
const url = `https://${SCRIPT}.${subdomain}.workers.dev`;
const call = async <T>(path: string, body: unknown): Promise<T> => {
  for (let attempt = 1; ; attempt++) {
    let err: string;
    try {
      const res = await fetch(url + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` } });
      if (res.ok) return res.json() as Promise<T>;
      err = `${res.status} ${await res.text()}`;
    } catch (e) { err = String(e); }
    if (attempt >= 6) throw new Error(`${path}: ${err}`);
    await new Promise((r) => setTimeout(r, 3000 * attempt)); // a fresh deploy can take a few seconds to serve
  }
};
await call("/seed", { bindings: buckets.map((b) => b.binding) });

// 3. Time every bucket from a probe in each region; batches stay under the subrequest limit.
// Six buckets a call keeps each request to about 60 sequential reads, well under a proxy's idle timeout.
const timing: Record<string, { colo: string; ms: Record<string, number | string> }> = {};
await Promise.all(HINTS.map(async (hint) => {
  const ms: Record<string, number | string> = {};
  let colo = "";
  for (let i = 0; i < buckets.length; i += 6) {
    const r = await call<{ colo: string; ms: Record<string, number | string> }>("/time", { hint, bindings: buckets.slice(i, i + 6).map((b) => b.binding) });
    colo = r.colo; Object.assign(ms, r.ms);
  }
  timing[hint] = { colo, ms };
  console.error(`probe ${hint} in ${colo}`);
}));

// 4. Each bucket's measured region is the probe with the fastest median read.
const rows = buckets.map((b) => {
  const byProbe = Object.fromEntries(HINTS.map((h) => [h, timing[h].ms[b.binding]]));
  const numeric = HINTS.filter((h) => typeof byProbe[h] === "number").sort((x, y) => (byProbe[x] as number) - (byProbe[y] as number));
  const measured = numeric[0] ?? "unknown";
  const margin = numeric.length > 1 ? (byProbe[numeric[1]] as number) - (byProbe[numeric[0]] as number) : null;
  return { bucket: b.name, hint: b.hint, apiLocation: apiLocation.get(b.name)!, measured, marginMs: margin, byProbe };
});
const agree = rows.filter((r) => r.apiLocation === r.measured);
const apiIsHint = rows.every((r) => r.apiLocation === r.hint);
// One probe per hint cannot split neighbouring regions on one continent; run/b0-13-edges.ts does that.
const CONTINENT: Record<Region, string> = { enam: "na", wnam: "na", weur: "eu", eeur: "eu", apac: "apac", oc: "oc" };
const agreeContinent = rows.filter((r) => CONTINENT[r.apiLocation as Region] === CONTINENT[r.measured as Region]);
const summary = {
  at: new Date().toISOString(), buckets: rows.length, probeColos: Object.fromEntries(HINTS.map((h) => [h, timing[h].colo])),
  apiAgreesWithProbe: agree.length, share: agree.length / rows.length, apiReportsHintForEvery: apiIsHint,
  apiAgreesWithProbeByContinent: agreeContinent.length,
  // The fail case is a bucket placed off its hint while the API still reports the hint. A bucket whose API location
  // equals its hint is that case only if the probe places it elsewhere, and one probe per hint resolves continents,
  // not neighbouring regions (run/b0-13-edges.ts).
  verdict: agree.length === rows.length ? "Pass"
    : agreeContinent.length === rows.length ? "Agrees by continent; neighbouring regions need run/b0-13-edges.ts"
    : apiIsHint ? "Fail: the API reports the hint, not the placement" : "Not 100%: see disagreements",
  byHint: Object.fromEntries(HINTS.map((h) => {
    const rs = rows.filter((r) => r.hint === h);
    const count = (k: "apiLocation" | "measured") => Object.fromEntries([...new Set(rs.map((r) => r[k]))].map((v) => [v, rs.filter((r) => r[k] === v).length]));
    return [h, { api: count("apiLocation"), measured: count("measured"), minMarginMs: Math.min(...rs.map((r) => r.marginMs ?? Infinity)) }];
  })),
  disagreements: rows.filter((r) => r.apiLocation !== r.measured),
};
mkdirSync(new URL("../results/raw/", import.meta.url), { recursive: true });
writeFileSync(new URL("../results/raw/b0-13.json", import.meta.url), JSON.stringify({ summary, rows }, null, 1));
writeFileSync(new URL("../results/b0-13.json", import.meta.url), JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
