// A row's setup before load: drives `/catchup` on one site's shard of the network through the test edge's bed route
// until it answers 200 (its catch-up or rehydrate report) and the shard's `/status` reports no rehydrate under way,
// each call waiting at most `wait` s and answering `warming` past it. The bed's call is a client's, so it starts a
// stopped container; a leadership's `warm` keep starts none (`workers/site/src/prsync.rs`, `warm`). On m3fr7 the
// rehomed apac shard was cold when the row started: its rehydrate ran 69 s under the row's own 1 MiB pushes, and every
// MergeQueue merge in that window answered `warming` (`tests/am1/results/lm1-rows-m3f.md`, Rows m3fr7, B0-31). On
// B0-33 m61 sites b and c started their shards only under load and rehydrated for 122.6 and 129.5 s, and 486 requests
// answered `warming` (`tests/am1/results/b033-m61-why.md`). One JSON line per call; exit 0 once warm, 2 past `limit`.
//   node tests/fanout/shardwarm.mjs <repo id> <site> [limit minutes 15] [wait seconds 60]
import { bedGet } from "../am1/hosted/lib.mjs";
const [id, site, limitMin = "15", waitS = "60"] = process.argv.slice(2);
const t0 = Date.now();
const until = t0 + Number(limitMin) * 60_000;
const fail = (e) => ({ status: 0, body: String(e).slice(0, 80) });
while (Date.now() < until) {
  const t = Date.now();
  const r = await bedGet(`/_bed/catchup?repo=${id}&shard=${site}&wait=${waitS}`).catch(fail);
  let v = r.body.slice(0, 160);
  try { const j = JSON.parse(r.body); v = { entries: j.entries, ms: j.ms, warming: j.warming, rehydrated: !!j.rehydrated }; } catch {}
  // A 200 can end a rehydrate's first run while its place is kept until the tail reaches the head
  // (`ShardHost::rehydrating`): only a shard that reports none is warm.
  let rehydrating = null;
  if (r.status === 200) {
    const s = await bedGet(`/_bed/shard_status?repo=${id}&shard=${site}`).catch(fail);
    try { rehydrating = JSON.parse(s.body).rehydrating ?? null; } catch {}
  }
  console.log(JSON.stringify({ at: new Date().toISOString(), site: Number(site), status: r.status, ms: Date.now() - t, rehydrating, v }));
  if (r.status === 200 && rehydrating === false) {
    console.log(`warm site ${site} after ${((Date.now() - t0) / 1000).toFixed(1)} s`);
    process.exit(0);
  }
  await new Promise((d) => setTimeout(d, 5_000));
}
console.log(`NOT warm site ${site} after ${limitMin} min`);
process.exit(2);
