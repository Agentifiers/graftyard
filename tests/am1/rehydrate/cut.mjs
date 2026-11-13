// rehydrate-batch's cut-off rehydrate, hosted: kills repo $HOT_ID's first shard on ABORT_SITE and KILL_SITE (default 0
// and 1) so both rehydrate on new disks, waits until each is CUT_S seconds into its run (default 60: past the marked
// phase, inside the tail), then cuts each. ABORT_SITE's shard object is reset (`/_bed/shard_abort`, test-faults
// `ctx.abort()`) while its container keeps its disk, so its alarm restarts the run from the saved place
// ("resumed": true in its "shard rehydrate" log line). KILL_SITE's container is torn down again (`/_bed/kill`), a new
// disk, so the next call rehydrates from nothing. Then polls both until a catch-up answers 200, and prints the times.
//   ABORT_SITE=0 KILL_SITE=1 CUT_S=60 node tests/am1/rehydrate/cut.mjs
import { bedGet, log, sleep } from "../hosted/lib.mjs";

const ID = process.env.HOT_ID;
const A = Number(process.env.ABORT_SITE ?? 0), K = Number(process.env.KILL_SITE ?? 1);
const CUT = Number(process.env.CUT_S ?? 60) * 1000;
const call = (p) => bedGet(p).catch((e) => ({ status: 0, body: String(e) }));
const t0 = Date.now();
for (const s of [A, K]) log(`cut kill shard ${s}: ${(await call(`/_bed/kill?repo=${ID}&shard=${s}`)).status}`);
// A first catch-up waits out its container's start before `wait` applies; it is not awaited here.
for (const s of [A, K]) call(`/_bed/catchup?repo=${ID}&shard=${s}&wait=5`).then((r) => log(`cut start shard ${s}: ${r.status}`));
await sleep(CUT);
const a = await call(`/_bed/shard_abort?repo=${ID}&shard=${A}`);
log(`cut abort shard ${A} at ${((Date.now() - t0) / 1000).toFixed(0)} s: ${a.status} ${a.body.slice(0, 60)}`);
const k = await call(`/_bed/kill?repo=${ID}&shard=${K}`);
log(`cut kill again shard ${K} at ${((Date.now() - t0) / 1000).toFixed(0)} s: ${k.status} ${k.body.slice(0, 60)}`);
const left = new Set([A, K]);
while (left.size) {
  await sleep(30_000);
  for (const s of [...left]) {
    const r = await call(`/_bed/catchup?repo=${ID}&shard=${s}&wait=60`);
    log(`cut shard ${s} at ${((Date.now() - t0) / 1000).toFixed(0)} s: ${r.status} ${r.body.slice(0, 120)}`);
    if (r.status === 200) left.delete(s);
  }
}
log("cut done");
