// LM-1 round 17: the row's memory guard. Every `every` seconds, each voter's `/stats?summary=1` (`wasmMemMb`,
// `memSplit`, role, term), one JSON line per sample to stdout. Once a leader's wasm memory passes `limit` MiB, it
// writes STOP to stderr and SIGTERMs every `a1.mjs` on this orb, so a row's numbers never come from a memory storm
// (the parent's 120 MiB stop line; narrow-reads-3's gate 16 rose from 77 to 258 MiB in one minute). Read only.
//   node tests/fanout/memguard.mjs <repo name> <seconds> [every 10] [limit 120]
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { member, voterNames, sleep } from "../am1/hosted/lib.mjs";
const [name, secs, every = "10", limit = "120"] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
const chain = `repo:${id}`;
let names = {};
let stopped = false;
while (Date.now() < until) {
  const t = Date.now();
  const got = await voterNames(chain).catch(() => ({}));
  if (Object.keys(got).length) names = got;
  const st = await Promise.all([0, 1, 2].map(async (s) => {
    const v = (await member(chain, s, "/stats?summary=1", undefined, 8_000, names[s]).catch(() => ({}))).v ?? {};
    return { site: s, role: v.role ?? null, mib: v.wasmMemMb == null ? null : +(v.wasmMemMb * 1e6 / 1048576).toFixed(1),
      memSplit: v.memSplit ?? null };
  }));
  console.log(JSON.stringify({ at: new Date(t).toISOString(), members: st }));
  const lead = st.find((m) => m.role === true);
  if (!stopped && lead && lead.mib > Number(limit)) {
    stopped = true;
    console.error(`${new Date().toISOString()} STOP: leader site ${lead.site} at ${lead.mib} MiB > ${limit}`);
    try { execFileSync("pkill", ["-TERM", "-f", "tests/am1/rest/a1.mjs"]); } catch {}
  }
  await sleep(Math.max(0, Number(every) * 1000 - (Date.now() - t)));
}
