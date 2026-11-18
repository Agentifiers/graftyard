// Which repository chains on the install still have a joined member: for each id in the ranges given, each site's
// registry row (tools `/registry`) and first-generation voter get one `/status`; a chain with no registry row on any
// site and no joined first-generation voter is skipped. Prints one line per chain with a joined member, then a count.
// Read-only. Fold-audit leak check (operator, 2026-11-18: every joined test repository refolds once a day).
//   node tests/fanout/joinedm2b.mjs 65536-65600 131072-131360 327680-327710 9043968-9043975
import { member, tools } from "../am1/hosted/lib.mjs";

const ids = process.argv.slice(2).flatMap((a) => {
  const [x, y] = a.split("-").map(Number);
  return y ? Array.from({ length: y - x + 1 }, (_, i) => x + i) : [x];
});
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 60) }));
const q = [...ids];
const joined = [];
let rows = 0;
await Promise.all(Array.from({ length: 6 }, async () => {
  while (q.length) {
    const id = q.shift();
    const chain = `repo:${id}`;
    const names = new Map();
    for (const s of [0, 1, 2]) {
      names.set(`${chain}#1#f${s}`, s);
      const row = await tools("/registry", { chain, site: s }).catch(() => null);
      if (row?.do_name) { names.set(row.do_name, s); rows++; }
    }
    const left = [];
    for (const [n, s] of names) {
      const st = await status(chain, s, n);
      if (st?.joined !== false) left.push(`${n} ${st?.error ? `error ${st.error}` : `${st?.role} t${st?.term} c${st?.commit}`}`);
    }
    if (left.length) { joined.push(id); console.log(`${chain}: ${left.join("; ")}`); }
  }
}));
console.log(`checked ${ids.length} ids (${rows} registry rows); with a joined or unreadable member: ${joined.length} ${JSON.stringify(joined.sort((a, b) => a - b))}`);
