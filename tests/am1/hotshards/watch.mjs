// hot-shards-4's kill rerun watch: every 5 s, each site's git cache shard `/status` of the network (whether its
// rehydrate's place is kept, `rehydrating`, and the cursor its latest catch-up reached) and the group's leader commit
// index, so the run shows the mark holds from the kill until the shard's cursor reaches the head. One JSON line per
// sample to stdout. A member logs each `/entries` page it caps at 1 MB under load ("entries page capped").
//   SECONDS=1200 node tests/am1/hotshards/watch.mjs
import { bedGet, sleep, statuses } from "../hosted/lib.mjs";

const CHAIN = process.env.HOT_CHAIN, ID = process.env.HOT_ID;
if (!CHAIN || !ID) throw new Error("HOT_CHAIN and HOT_ID");
const end = Date.now() + Number(process.env.SECONDS ?? 1200) * 1000;
while (Date.now() < end) {
  const at = new Date().toISOString();
  const shards = await Promise.all([0, 1, 2].map(async (s) => {
    const r = await bedGet(`/_bed/shard_status?repo=${ID}&shard=${s}`).catch((e) => ({ status: 0, body: String(e) }));
    try {
      const v = JSON.parse(r.body);
      return { site: s, running: v.running, rehydrating: v.rehydrating, cursor: v.cursor };
    } catch {
      return { site: s, status: r.status };
    }
  }));
  const st = await statuses(CHAIN);
  const lead = st.find((s) => s.role === "leader");
  const row = { at, commit: lead?.commit ?? lead?.commitIndex ?? null, leader: lead?.site ?? null, shards };
  console.log(JSON.stringify(row));
  await sleep(5_000);
}
