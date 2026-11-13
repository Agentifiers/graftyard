// hot-shards-3's A31 run on graft-m3c: does derived work alone (rehydrate, catch-up, repack, promotion) raise
// `fanout`'s k? Forces k = 1 on the group's leader (test-faults `/bench_seed {"fanout": 1}`), kills every site's first
// shard of the network so each rehydrates and catches up with no pushes, then each minute for MINUTES (default 12,
// past FANOUT_UP_FOR's 3 min) records the committed k (the members' `ranges` read) and each site's first shard
// `/status` (`load`: the container's NetworkLoad with derived work left out; `k`: what the shard last read). No push
// runs, and a first shard reports its load only on its own push validates (`workers/site/src/shard.rs`), so k
// cannot step here at all; what this run measures is the load a report would carry while derived work runs. One JSON line a minute in
// tests/am1/raw/hotshards3/a31.jsonl. Needs tests/am1/lf13/env-m3c.sh sourced, HOT_CHAIN and HOT_ID.
//   node tests/am1/hotshards/a31.mjs
import { appendFileSync, mkdirSync } from "node:fs";
import { bedGet, leaderOf, log, member, sleep } from "../hosted/lib.mjs";

const CHAIN = process.env.HOT_CHAIN, ID = process.env.HOT_ID;
if (!CHAIN || !ID) throw new Error("HOT_CHAIN and HOT_ID");
const OUT = "tests/am1/raw/hotshards3/a31.jsonl";
const MINUTES = Number(process.env.MINUTES ?? 12);
mkdirSync("tests/am1/raw/hotshards3", { recursive: true });
const row = (r) => appendFileSync(OUT, JSON.stringify({ at: new Date().toISOString(), ...r }) + "\n");

const leader = await leaderOf(CHAIN);
const forced = await member(CHAIN, leader, "/bench_seed", { fanout: 1 }, 20_000).catch((e) => ({ v: String(e) }));
row({ event: "force", leader, v: forced.v });
if (process.env.NO_KILL !== "1") {
  for (const s of [0, 1, 2]) {
    const r = await bedGet(`/_bed/kill?repo=${ID}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
    row({ event: "kill", site: s, status: r.status, body: r.body.slice(0, 120) });
  }
  // A warm-up call per site so each shard starts its rehydrate now (a stopped shard starts on its first client call).
  // Not awaited: `/_bed/catchup` waits for the catch-up, which goes on in the shard object either way.
  for (const s of [0, 1, 2]) {
    bedGet(`/_bed/catchup?repo=${ID}&shard=${s}`)
      .then((r) => row({ event: "caught up", site: s, status: r.status, body: r.body.slice(0, 300) }))
      .catch((e) => row({ event: "catchup call", site: s, error: String(e).slice(0, 80) }));
  }
}
const t0 = Date.now();
for (let m = 0; m <= MINUTES; m++) {
  await sleep(Math.max(0, t0 + m * 60_000 - Date.now()));
  const ks = await Promise.all([0, 1, 2].map(async (s) => {
    try {
      const v = (await member(CHAIN, s, "/read", { q: { ranges: true } }, 15_000)).v;
      return v?.value?.fanout ?? v?.fanout ?? null;
    } catch (e) { return String(e).slice(0, 60); }
  }));
  const shards = await Promise.all([0, 1, 2].map(async (s) => {
    try {
      const r = await bedGet(`/_bed/shard_status?repo=${ID}&shard=${s}`);
      const v = JSON.parse(r.body);
      return { site: s, running: v.running, k: v.k, load: v.load };
    } catch (e) { return { site: s, error: String(e).slice(0, 80) }; }
  }));
  row({ minute: m, committedK: ks, shards });
  log(`a31 minute ${m}: k ${JSON.stringify(ks)} loads ${shards.map((s) => JSON.stringify(s.load?.cpu_p99 ?? s.load ?? s.error)).join(" ")}`);
}
log("a31 done");
