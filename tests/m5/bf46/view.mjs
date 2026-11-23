// BF-46's read of graft-m4a's `Release` group: every site's `/view` from position 0 through the tools edge, the
// longest one replayed and checked against the install's anchor by `relview` (tests/m3/hosted/tools). Usage, with the
// graft-m4a env sourced (tests/am1/lf13/env-m4a.sh) and RELVIEW the built binary:
//   node tests/m5/bf46/view.mjs prev               {"prev": <last digest>, "count": n}
//   node tests/m5/bf46/view.mjs entries <since ms>  the entries committed since then, the current intent and slots
//   node tests/m5/bf46/view.mjs slots              every secret's active slot
//   node tests/m5/bf46/view.mjs pages <file>       BF-47: the raw pages for `release roll --entries`, into <file>
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { member } from "../../am1/hosted/lib.mjs";

const toml = readFileSync(`${process.env.HOME}/.graft-m4a/install.toml`, "utf8");
const field = (k) => toml.match(new RegExp(`^${k}\\s*=\\s*"([^"]*)"`, "m"))[1];
const [cmd, arg] = process.argv.slice(2);

async function pages(site) {
  const out = [];
  for (let from = 0; ; ) {
    const r = await member("release", site, "/view", { from });
    const v = r.v ?? r;
    if (!v?.entries) throw new Error(`site ${site}: ${JSON.stringify(r).slice(0, 200)}`);
    out.push({ count: v.count, entries: v.entries });
    from += v.entries.length;
    if (!v.entries.length || from >= v.count) return out;
  }
}
const all = await Promise.allSettled([0, 1, 2].map(pages));
const best = all.filter((p) => p.status === "fulfilled").map((p) => p.value)
  .sort((a, b) => b.reduce((n, p) => n + p.entries.length, 0) - a.reduce((n, p) => n + p.entries.length, 0))[0];
if (!best) throw new Error("no site answered /view");
const out = JSON.parse(execFileSync(process.env.RELVIEW, [field("install"), field("customer_key"), "0"],
  { input: JSON.stringify(best), maxBuffer: 64 << 20 }).toString());
if (cmd === "prev") console.log(JSON.stringify({ prev: out.prev, count: out.count }));
else if (cmd === "slots") console.log(JSON.stringify({ count: out.count, current: out.current, slots: out.slots }));
else if (cmd === "pages") {
  // BF-47: the pages `graftyard release roll --entries` reads, written to <arg>, and the record they replay to.
  (await import("node:fs")).writeFileSync(arg, JSON.stringify(best));
  console.log(JSON.stringify({ count: out.count, current: out.current, site_release: out.site_release, rolled: out.rolled }));
} else if (cmd === "entries") {
  const since = Number(arg ?? 0);
  const entries = out.entries.filter((e) => e.at >= since);
  console.log(JSON.stringify({ count: out.count, current: out.current, drained: out.drained, slots: out.slots }));
  for (const e of entries) console.log(JSON.stringify(e));
} else throw new Error("view.mjs prev | entries <since ms> | slots | pages <file>");
