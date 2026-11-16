// B0-31's setup after the rehome: drives `/catchup` on one site's shard of the network through the test edge's bed
// route until it answers 200 (its catch-up or rehydrate report), each call waiting at most `wait` s and answering
// `warming` past it. On m3fr7 the rehomed apac shard was cold when the row started: its rehydrate ran 69 s under the
// row's own 1 MiB pushes, and every MergeQueue merge in that window answered `warming` after MERGE_WAIT
// (`tests/am1/results/lm1-rows-m3f.md`, Rows m3fr7, B0-31). One JSON line per call; exit 0 once warm, 2 past `limit`.
//   node tests/fanout/shardwarm.mjs <repo id> <site> [limit minutes 15] [wait seconds 60]
import { bedGet } from "../am1/hosted/lib.mjs";
const [id, site, limitMin = "15", waitS = "60"] = process.argv.slice(2);
const t0 = Date.now();
const until = t0 + Number(limitMin) * 60_000;
while (Date.now() < until) {
  const t = Date.now();
  const r = await bedGet(`/_bed/catchup?repo=${id}&shard=${site}&wait=${waitS}`).catch((e) => ({ status: 0, body: String(e).slice(0, 80) }));
  let v = r.body.slice(0, 160);
  try { const j = JSON.parse(r.body); v = { entries: j.entries, ms: j.ms, warming: j.warming }; } catch {}
  console.log(JSON.stringify({ at: new Date().toISOString(), site: Number(site), status: r.status, ms: Date.now() - t, v }));
  if (r.status === 200) {
    console.log(`warm site ${site} after ${((Date.now() - t0) / 1000).toFixed(1)} s`);
    process.exit(0);
  }
  await new Promise((d) => setTimeout(d, 5_000));
}
console.log(`NOT warm site ${site} after ${limitMin} min`);
process.exit(2);
