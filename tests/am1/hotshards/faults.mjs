// hot-shards' faults during the journey-9 phase on graft-m3c: leader changes by member `/transfer` (as
// tests/am1/b030/load.mjs does) and one shard reset (`/_bed/kill` of the leader site's shard, which tears its container
// down, so its next catch-up rehydrates on a new disk). Each fault is logged with its time, so the push rows and the
// monitor rows around it can be read against it.
//
//   AT="10:transfer,25:kill,40:transfer" node tests/am1/hotshards/faults.mjs   # minutes after start
import { bedGet, log, member, sleep, statuses } from "../hosted/lib.mjs";

const CHAIN = process.env.HOT_CHAIN, ID = process.env.HOT_ID;
if (!CHAIN || !ID) throw new Error("HOT_CHAIN and HOT_ID");
const plan = (process.env.AT ?? "10:transfer,25:kill,40:transfer").split(",").map((s) => {
  const [m, what] = s.split(":");
  return { at: Number(m) * 60_000, what };
});
const t0 = Date.now();

async function leader() {
  for (let i = 0; i < 30; i++) {
    const l = (await statuses(CHAIN)).find((s) => s.role === "leader");
    if (l) return l;
    await sleep(2_000);
  }
  return null;
}

for (const f of plan) {
  await sleep(Math.max(0, t0 + f.at - Date.now()));
  const l = await leader();
  if (!l) { log(`hot fault ${f.what}: no leader`); continue; }
  if (f.what === "transfer") {
    const to = (l.site + 1) % 3;
    const r = await member(CHAIN, l.site, "/transfer", { to }, 15_000).catch((e) => ({ v: String(e).slice(0, 80) }));
    log(`hot fault transfer ${l.site}->${to} term ${l.term}: ${JSON.stringify(r.v?.transfer ?? r.v).slice(0, 80)}`);
    const n = await leader();
    log(`hot fault transfer done: leader ${n?.site} term ${n?.term}`);
  } else if (f.what === "kill") {
    const r = await bedGet(`/_bed/kill?repo=${ID}&shard=${l.site}`).catch((e) => ({ status: 0, body: String(e) }));
    log(`hot fault kill shard ${l.site}: ${r.status} ${r.body.slice(0, 120)}`);
  }
}
log("hot faults done");
