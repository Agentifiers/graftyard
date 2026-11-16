// B0-30 rerun, before `/retire` (operator, 2026-11-16): every live member's final /status, /stats and /bench_seed size
// to tests/am1/raw/b030r2-final.jsonl, and each parent voter's log around entry 208,160 (the stalled `Degraded`) to
// tests/am1/raw/b030r2-log208160.jsonl, so the evidence outlives the objects. Read-only.
import { appendFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";

const R = "tests/am1/raw";
const MEMBERS = [
  ["repo:1048590", 0, "repo:1048590#1#f0"], ["repo:1048590", 1, "repo:1048590#5#f1"], ["repo:1048590", 2, "repo:1048590#1#f2"],
  ["repo-meta:1048590", 0, "repo-meta:1048590#1#f0"], ["repo-meta:1048590", 1, "repo-meta:1048590#5#f1"], ["repo-meta:1048590", 2, "repo-meta:1048590#1#f2"],
];
const get = (chain, site, path, body, name) => member(chain, site, path, body, 60_000, name).then((r) => r.v ?? { error: `no value: ${JSON.stringify(r).slice(0, 200)}` }, (e) => ({ error: String(e).slice(0, 200) }));
const only = process.argv[2] ? Number(process.argv[2]) : 0;
for (const [chain, site, name] of MEMBERS.slice(only)) {
  const row = { at: new Date().toISOString(), chain, site, name, status: await get(chain, site, "/status", undefined, name),
    stats: await get(chain, site, "/stats", undefined, name), size: await get(chain, site, "/bench_seed", { b030: { size: true } }, name) };
  appendFileSync(`${R}/b030r2-final.jsonl`, JSON.stringify(row) + "\n");
  console.log(name, row.status.role ?? row.status.error, "commit", row.status.commit, "db", row.size.dbBytes);
  if (chain === "repo:1048590") {
    const log = await get(chain, site, "/log?from=208150&n=30", undefined, name);
    for (const e of Array.isArray(log) ? log : [log]) { delete e.packs; appendFileSync(`${R}/b030r2-log208160.jsonl`, JSON.stringify({ site, name, ...e }) + "\n"); }
  }
}
