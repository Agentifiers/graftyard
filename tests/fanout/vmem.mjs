// Every voter's memory beside a row, for L0-12's storm reproducer (parent's ask after m3fr2: voters' isolates reset on
// memory under L0-12's load): every EVERY ms (default 2,000), each live voter's `/stats?summary=1` wasm high-water
// (`wasmMemMb`), log bytes, held bytes, and memory split (`memSplit`), one JSON line per voter, until `seconds` pass.
// tests/nr3/lmem.mjs samples the leader alone; a reset here was as often a follower's. No reset of the stats.
//   node tests/fanout/vmem.mjs <repo name> <seconds>   -> stdout
// Waits for the repository's id in am1-runs-repos.txt (rows.sh creates it). Environment as tests/am1/hosted/lib.mjs.
import { readFileSync } from "node:fs";
import { member, sleep, voterNames } from "../am1/hosted/lib.mjs";

const [name, secs] = process.argv.slice(2);
const every = Number(process.env.EVERY ?? 2_000);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
if (!id) throw new Error(`${name}: no id`);
const chain = `repo:${id}`;
let names = {};
// Election rows (`/elections`, test-faults) every ELECT_EVERY ms per voter, since an isolate reset loses them: one
// `{ elect: [[at, kind, term, peer, index, extra], ...] }` line per voter per read.
const electEvery = Number(process.env.ELECT_EVERY ?? 20_000);
const since = [0, 0, 0];
let lastElect = 0;
while (Date.now() < until) {
  const t = Date.now();
  const got = await voterNames(chain).catch(() => ({}));
  if (Object.keys(got).length) names = got;
  if (t - lastElect >= electEvery) {
    lastElect = t;
    await Promise.all([0, 1, 2].map(async (site) => {
      try {
        const rows = (await member(chain, site, `/elections?since=${since[site]}`, undefined, 5_000, names[site])).v?.rows ?? [];
        if (rows.length) since[site] = rows[rows.length - 1][0];
        console.log(JSON.stringify({ at: new Date(t).toISOString(), site, name: names[site], elect: rows }));
      } catch (e) {
        console.log(JSON.stringify({ at: new Date(t).toISOString(), site, electError: String(e).slice(0, 100) }));
      }
    }));
  }
  await Promise.all([0, 1, 2].map(async (site) => {
    try {
      const v = (await member(chain, site, "/stats?summary=1", undefined, 5_000, names[site])).v ?? {};
      console.log(JSON.stringify({ at: new Date(t).toISOString(), site, leader: v.role, wasmMemMb: v.wasmMemMb, logBytes: v.logBytes,
        held: Array.isArray(v.held) ? v.held.reduce((n, h) => n + (h[2] ?? 0), 0) : v.held, memSplit: v.memSplit }));
    } catch (e) {
      console.log(JSON.stringify({ at: new Date(t).toISOString(), site, error: String(e).slice(0, 100) }));
    }
  }));
  await sleep(Math.max(0, every - (Date.now() - t)));
}
