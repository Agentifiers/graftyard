// hot-shards: holds promotion and archiving off while the seed builds the log above the last `Promoted`, by setting
// each member's test-faults `archiveCap` every HOLD_S seconds (default 15). The cap lives in the member's isolate, so a
// reset member loses it; the seed's own two-minute re-cap let promote-ahead run in between (graft-m3c, 10:27Z). Stops
// when tests/am1/raw/hotshards/UNHOLD exists, then clears the cap on every member.
import { existsSync } from "node:fs";
import { log, member, sleep, voterNames } from "../hosted/lib.mjs";

const CHAIN = process.env.HOT_CHAIN;
const EVERY = Number(process.env.HOLD_S ?? 15) * 1000;
const UNHOLD = "tests/am1/raw/hotshards/UNHOLD";
let names = {};
const set = (cap) => Promise.all([0, 1, 2].map((s) => member(CHAIN, s, "/bench_seed", { archiveCap: cap }, 15_000, names[s])
  .then((r) => r.v?.archiveCap ?? null).catch((e) => String(e).slice(0, 40))));
let n = 0;
while (!existsSync(UNHOLD)) {
  names = { ...names, ...(await voterNames(CHAIN).catch(() => ({}))) };
  const r = await set({ max: 100e9, range: 2 << 20 });
  if (n++ % 20 === 0) log(`hot hold: ${JSON.stringify(r)}`);
  await sleep(EVERY);
}
log(`hot unhold: ${JSON.stringify(await set({}))}`);
