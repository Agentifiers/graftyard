// Stands in for the edge's `/suspect` while no client calls a group: campaigns on the group's home member once the
// group has had no leader for KEEP_AFTER_S (default 30 s). Campaigns start only from requests (`workers/site/src/host.rs`,
// header), and B0-30's load records its agent rows straight into the leader through `/bench_seed`, which never
// campaigns. Each campaign is a row in tests/am1/raw/b030split-keeper.jsonl.
//   node tests/am1/b030split/keeper.mjs [chain]
import { readFileSync } from "node:fs";
import { log, member, row, sleep, statuses } from "../hosted/lib.mjs";

const C = process.argv[2] ?? `repo:${JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo}`;
const AFTER = Number(process.env.KEEP_AFTER_S ?? 30) * 1000;
const OUT = "tests/am1/raw/b030split-keeper.jsonl";
let since = null;
for (;;) {
  const st = await statuses(C).catch(() => []);
  const led = st.some((s) => s.role === "leader");
  if (led) since = null;
  else {
    since ??= Date.now();
    if (Date.now() - since >= AFTER) {
      const home = st.find((s) => s.config?.config?.normal)?.config.config.normal.home ?? 0;
      const v = await member(C, home, "/campaign", {}, 30_000).catch((e) => ({ v: { error: String(e).slice(0, 200) } }));
      const r = { at: new Date().toISOString(), chain: C, leaderless_s: Math.round((Date.now() - since) / 1000), site: home, terms: st.map((s) => s.term), verdict: v.v?.verdict ?? v.v };
      row(OUT, r);
      log(`campaign ${JSON.stringify(r)}`);
      since = Date.now();
    }
  }
  await sleep(10_000);
}
