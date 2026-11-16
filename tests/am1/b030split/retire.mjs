// B0-30 rerun teardown (operator, 2026-11-16, option (a)): clears each live member of repo:1048590 and its child
// repo-meta:1048590 with the test route `/retire` (BF-11's lost-member stub: storage deleted, object reset), as smoke
// 1's repo:1048586 was retired. Each member by its current voter name. Then `/status` on each: "not joined" is done.
// Final readings were saved first (final.mjs). R2 archives and snapshots stay.
import { appendFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";

const LOG = "tests/am1/raw/b030-retire-1048590.log";
const log = (s) => { const l = `${new Date().toISOString()} ${s}`; console.log(l); appendFileSync(LOG, l + "\n"); };
const MEMBERS = process.argv[2] === "f1-gen1" ? [["repo:1048590", 1, "repo:1048590#1#f1"], ["repo-meta:1048590", 1, "repo-meta:1048590#1#f1"]] : [
  ["repo:1048590", 0, "repo:1048590#1#f0"], ["repo:1048590", 1, "repo:1048590#5#f1"], ["repo:1048590", 2, "repo:1048590#1#f2"],
  ["repo-meta:1048590", 0, "repo-meta:1048590#1#f0"], ["repo-meta:1048590", 1, "repo-meta:1048590#5#f1"], ["repo-meta:1048590", 2, "repo-meta:1048590#1#f2"],
];
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
const brief = (v) => v?.joined === false ? "not joined" : v?.error ? `error ${v.error}` : `${v?.role} t${v?.term} c${v?.commit}`;
for (const [c, s, n] of MEMBERS) {
  log(`${n} before: ${brief(await status(c, s, n))}`);
  const r = await member(c, s, "/retire", {}, 60_000, n).catch((e) => ({ status: String(e).slice(0, 80) }));
  log(`${n} /retire: ${JSON.stringify(r?.v ?? r?.status ?? null).slice(0, 80)}`);
}
await new Promise((r) => setTimeout(r, 15_000));
for (const [c, s, n] of MEMBERS) log(`${n} after: ${brief(await status(c, s, n))}`);
