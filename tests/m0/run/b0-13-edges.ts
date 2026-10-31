// B0-13 refinement. One probe per hint cannot separate neighbouring regions when the probe lands in a central colo
// (ORD for enam, FRA for eeur). This places many probes per hint, keeps one probe per colo, and times the buckets of
// the neighbouring hints from every colo. Each bucket's nearest colo then says where it is.
//
//   node run/b0-13-edges.ts    (after run/b0-13.ts has created the buckets and deployed graft-m0-probe)

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";

const TOKEN = readFileSync(new URL("../.m0-token", import.meta.url), "utf8").trim();
const URL_ = "https://graft-m0-probe.<workers-subdomain>.workers.dev";
const PAIRS = [["enam", "wnam"], ["weur", "eeur"]] as const;
const PROBES_PER_HINT = 16;
const PER = 20;

execFileSync("npx", ["wrangler", "deploy", "-c", ".gen/probe.jsonc"], { stdio: ["ignore", "ignore", "inherit"] });

async function call<T>(path: string, body: unknown): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    let err: string;
    try {
      const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` } });
      if (res.ok) return res.json() as Promise<T>;
      err = `${res.status} ${await res.text()}`;
    } catch (e) { err = String(e); }
    if (attempt >= 6) throw new Error(`${path}: ${err}`);
    await new Promise((r) => setTimeout(r, 3000 * attempt));
  }
}

const out: Record<string, unknown> = {};
for (const pair of PAIRS) {
  // 1. Where do the probes of each hint land? Keep the first probe in each colo.
  const probes = new Map<string, { name: string; hint: string }>();
  const coloHints: Record<string, string[]> = {};
  for (const hint of pair) {
    const found = await Promise.all(Array.from({ length: PROBES_PER_HINT }, (_, i) =>
      call<{ colo: string }>("/colo", { hint, name: `edge-${hint}-${i}` }).then((r) => ({ name: `edge-${hint}-${i}`, hint, colo: r.colo }))));
    for (const p of found) {
      (coloHints[p.colo] ??= []).includes(hint) || coloHints[p.colo].push(hint);
      if (!probes.has(p.colo)) probes.set(p.colo, { name: p.name, hint });
    }
  }
  console.error(`${pair.join("/")}: probe colos ${JSON.stringify(coloHints)}`);

  // 2. Time every bucket of both hints from every colo.
  const bindings = pair.flatMap((h) => Array.from({ length: PER }, (_, i) => `B_${h.toUpperCase()}_${i}`));
  const timing: Record<string, Record<string, number | string>> = {};
  await Promise.all([...probes].map(async ([colo, p]) => {
    timing[colo] = {};
    for (let i = 0; i < bindings.length; i += 6)
      Object.assign(timing[colo], (await call<{ ms: Record<string, number | string> }>("/time", { hint: p.hint, name: p.name, bindings: bindings.slice(i, i + 6) })).ms);
  }));

  // 3. Nearest colo per bucket, and the hints whose probes landed in that colo.
  const rows = bindings.map((b) => {
    const ranked = Object.entries(timing).filter(([, t]) => typeof t[b] === "number").sort((x, y) => (x[1][b] as number) - (y[1][b] as number));
    const [nearest, next] = ranked;
    const hint = b.split("_")[1].toLowerCase();
    return { binding: b, hint, nearest: nearest[0], nearestMs: nearest[1][b], marginMs: (next[1][b] as number) - (nearest[1][b] as number),
             nearestColoHints: coloHints[nearest[0]], consistent: coloHints[nearest[0]].includes(hint) };
  });
  out[pair.join("/")] = {
    probeColos: coloHints,
    consistent: rows.filter((r) => r.consistent).length, of: rows.length,
    nearestByHint: Object.fromEntries(pair.map((h) => [h, Object.fromEntries(Object.entries(Object.groupBy(rows.filter((r) => r.hint === h), (r) => r.nearest)).map(([c, rs]) => [c, rs!.length]))])),
    rows,
  };
  console.error(`${pair.join("/")}: ${rows.filter((r) => r.consistent).length}/${rows.length} nearest a colo of their own hint`);
}
writeFileSync(new URL("../results/raw/b0-13-edges.json", import.meta.url), JSON.stringify(out, null, 1));
console.log(JSON.stringify(Object.fromEntries(Object.entries(out).map(([k, v]) => [k, { ...(v as object), rows: undefined }])), null, 2));
