// rehydrate-batch's cold rehydrate on graft-m3f: once the seed has built the log above the last `Promoted`, tears down
// repo $HOT_ID's first shard on every site (`/_bed/kill`, a new disk), starts each one's catch-up
// (`/_bed/catchup?wait=5`, which answers 503 `warming` and goes on in the shard object), then polls each shard's
// `/_bed/catchup?wait=60` until it answers 200 with a report. Prints each site's report: `rehydrated` (packs, bytes,
// calls, resumed), `ms` (each lap's end), `held_max`, and the wall time from the kill. The Workers Logs line
// "shard rehydrate <id>" carries the same report (tests/am1/hotshards/logs.py).
//   SITES=0,1,2 node tests/am1/rehydrate/cold.mjs
import { bedGet, log, sleep } from "../hosted/lib.mjs";

const ID = process.env.HOT_ID;
const SITES = (process.env.SITES ?? "0,1,2").split(",").map(Number);
if (!ID) throw new Error("HOT_ID");
const t0 = {};
for (const s of SITES) {
  const r = await bedGet(`/_bed/kill?repo=${ID}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
  t0[s] = Date.now();
  log(`cold kill shard ${s}: ${r.status} ${r.body.slice(0, 80)}`);
}
// All at once: a first call waits out its container's start (about 2 min hosted) before `wait` applies.
await Promise.all(SITES.map(async (s) => {
  const r = await bedGet(`/_bed/catchup?repo=${ID}&shard=${s}&wait=5`).catch((e) => ({ status: 0, body: String(e) }));
  log(`cold start shard ${s}: ${r.status} ${r.body.slice(0, 80)}`);
}));
const left = new Set(SITES);
while (left.size) {
  await sleep(30_000);
  for (const s of [...left]) {
    const r = await bedGet(`/_bed/catchup?repo=${ID}&shard=${s}&wait=60`).catch((e) => ({ status: 0, body: String(e) }));
    if (r.status !== 200) {
      log(`cold shard ${s}: ${r.status} ${r.body.slice(0, 80)} at ${((Date.now() - t0[s]) / 1000).toFixed(0)} s`);
      continue;
    }
    let j;
    try { j = JSON.parse(r.body); } catch { j = { raw: r.body.slice(0, 200) }; }
    // A catch-up that ran after the rehydrate answers `rehydrated: null`; the rehydrate's own report is in the logs.
    log(`cold done shard ${s} at ${((Date.now() - t0[s]) / 1000).toFixed(0)} s: ${JSON.stringify({ rehydrated: j.rehydrated, ms: j.ms, held_max: j.held_max, entries: j.entries, packs: j.packs, cursor: j.cursor?.chains })}`);
    left.delete(s);
  }
}
log("cold done");
