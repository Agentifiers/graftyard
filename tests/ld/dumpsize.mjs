// dumpsize.mjs CHAIN [SITE] : one packed whole-tier read through graft-m3c-tools-edge; prints its size and index
// and the bytes per table. The tools token is read from ~/.graft-m3c and never printed.
import { readFileSync } from "node:fs";
const TOK = readFileSync(`${process.env.HOME}/.graft-m3c/tools.token`, "utf8").trim();
const TOOLS = `https://graft-m3c-tools-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const [chain, site] = [process.argv[2], Number(process.argv[3] ?? 0)];
const t = Date.now();
const r = await fetch(`${TOOLS}/member`, { method: "POST", headers: { authorization: `Bearer ${TOK}`, "content-type": "application/json" },
  body: JSON.stringify({ chain, site, path: "/read", body: { q: { rows: true, ranges: true, packed: true } }, deadline: 30000, gen: 1 }) });
const j = await r.json();
const v = j.v ?? j;
const rows = JSON.parse(v.value);
const by = {};
for (const tier of ["core", "meta"]) for (const [tb, k, c] of rows[tier] ?? []) {
  const b = JSON.stringify([tb, k, c]).length; const e = (by[`${tier}.${tb}`] ??= [0, 0]); e[0]++; e[1] += b; }
console.log(JSON.stringify({ status: r.status, ms: Date.now() - t, bytes: v.value.length, index: rows.index }));
for (const [k, [n, b]] of Object.entries(by).sort((a, b) => b[1][1] - a[1][1])) console.log(k.padEnd(28), String(n).padStart(6), String(b).padStart(9));
