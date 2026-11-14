// LM-1 round 17: the row's memory guard. Every `every` seconds, each voter's `/stats?summary=1` (`wasmMemMb`,
// `memSplit`, role), one JSON line per sample to stdout. Once any voter's wasm memory passes `limit` MiB, it writes
// STOP to stderr, with every sample of the minute before, and SIGTERMs every `a1.mjs` on this orb, which ends its
// window and writes the row as `stopped`, so a row's numbers never come from a memory storm (the parent's 120 MiB
// stop line; narrow-reads-3's gate 16 rose from 77 to 258 MiB in one minute). Read only.
//   node tests/fanout/memguard.mjs <repo name> <seconds> [every 2] [limit 120]
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { member, voterNames, sleep } from "../am1/hosted/lib.mjs";
const [name, secs, every = "2", limit = "120"] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
const chain = `repo:${id}`;
let names = {};
let stopped = false;
const recent = [];
for (let k = 0; Date.now() < until; k++) {
  const t = Date.now();
  // The names change only on a replacement: refreshed every 30 samples, or while none are known.
  if (k % 30 === 0 || !Object.keys(names).length) {
    const got = await voterNames(chain).catch(() => ({}));
    if (Object.keys(got).length) names = got;
  }
  const st = await Promise.all([0, 1, 2].map(async (s) => {
    const v = (await member(chain, s, "/stats?summary=1", undefined, 5_000, names[s]).catch(() => ({}))).v ?? {};
    return { site: s, role: v.role ?? null, mib: v.wasmMemMb == null ? null : +(v.wasmMemMb * 1e6 / 1048576).toFixed(1),
      memSplit: v.memSplit ?? null };
  }));
  const row = { at: new Date(t).toISOString(), members: st };
  console.log(JSON.stringify(row));
  recent.push(row);
  while (recent.length && Date.parse(recent[0].at) < t - 60_000) recent.shift();
  const over = st.find((m) => m.mib > Number(limit));
  if (!stopped && over) {
    stopped = true;
    console.error(`${new Date().toISOString()} STOP: site ${over.site} (${over.role ? "leader" : "voter"}) at ${over.mib} MiB > ${limit}`);
    console.error(JSON.stringify({ minuteBefore: recent }));
    try { execFileSync("pkill", ["-TERM", "-f", "tests/am1/rest/a1.mjs"]); } catch {}
  }
  await sleep(Math.max(0, Number(every) * 1000 - (Date.now() - t)));
}
