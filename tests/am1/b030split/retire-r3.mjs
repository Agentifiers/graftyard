// Operator stop (2026-11-16 20:1xZ, the account over budget): the third run's teardown. Saves each live member's last
// `/status` and size (`/bench_seed {b030:{size:true}}`) to tests/am1/raw/b030r3-final.jsonl, then clears it with the
// test route `/retire` (BF-11's lost-member stub: storage deleted, object reset), as retire.mjs did for repo:1048590.
// Then `/status` on each: "not joined" is done. R2 archives and snapshots stay.
//   node tests/am1/b030split/retire-r3.mjs CHAIN,SITE,DO_NAME ...
import { appendFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";

const LOG = "tests/am1/raw/b030-retire-r3.log", FINAL = "tests/am1/raw/b030r3-final.jsonl";
const log = (s) => { const l = `${new Date().toISOString()} ${s}`; console.log(l); appendFileSync(LOG, l + "\n"); };
const MEMBERS = process.argv.slice(2).map((a) => { const [c, s, n] = a.split(","); return [c, Number(s), n]; });
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
const brief = (v) => v?.joined === false ? "not joined" : v?.error ? `error ${v.error}` : `${v?.role} t${v?.term} c${v?.commit}`;
for (const [c, s, n] of MEMBERS) {
  const st = await status(c, s, n);
  const size = st?.joined === false ? null : await member(c, s, "/bench_seed", { b030: { size: true } }, 30_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
  if (size) delete size.meta;
  appendFileSync(FINAL, JSON.stringify({ at: new Date().toISOString(), chain: c, site: s, name: n, status: st, size }) + "\n");
  log(`${n} before: ${brief(st)}`);
  if (st?.joined === false) continue;
  const r = await member(c, s, "/retire", {}, 60_000, n).catch((e) => ({ status: String(e).slice(0, 80) }));
  log(`${n} /retire: ${JSON.stringify(r?.v ?? r?.status ?? null).slice(0, 80)}`);
}
await new Promise((r) => setTimeout(r, 15_000));
for (const [c, s, n] of MEMBERS) log(`${n} after: ${brief(await status(c, s, n))}`);
