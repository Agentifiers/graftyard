// narrow-reads-3's trace saver (operator after gate 16): every EVERY ms (default 60,000) until DONE_FILE exists, each
// voter's test-faults `/trace?since=` rows after the last ones saved from it, one JSON line per call:
// {at, site, leader, rows}. A member whose isolate resets loses its `Stats::trace`, so these keep what came before;
// legs.py merges them into the run's trace (`v<site>` and the leader's `f<site>`).
//   DONE_FILE=path node tests/nr3/tsave.mjs repo:ID > tests/nr3/raw/tsave-TAG.jsonl
const { member, statuses, voterNames, sleep } = await import("../am1/hosted/lib.mjs");
const { existsSync } = await import("node:fs");
const chain = process.argv[2];
const every = Number(process.env.EVERY ?? 60_000);
const since = [0, 0, 0];
const done = () => process.env.DONE_FILE && existsSync(process.env.DONE_FILE);
for (;;) {
  const last = done();
  try {
    const st = await statuses(chain);
    const leader = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0]?.site ?? null;
    const names = await voterNames(chain).catch(() => ({}));
    for (const site of [0, 1, 2]) {
      try {
        const rows = (await member(chain, site, `/trace?since=${since[site]}`, undefined, 30_000, names[site])).v ?? [];
        // A reset member's clock moves on, so `since` stays valid; its new rows are all later.
        if (rows.length) since[site] = rows[rows.length - 1][0];
        console.log(JSON.stringify({ at: new Date().toISOString(), site, leader, rows }));
      } catch (e) {
        console.log(JSON.stringify({ at: new Date().toISOString(), site, error: String(e).slice(0, 120) }));
      }
    }
  } catch (e) {
    console.log(JSON.stringify({ at: new Date().toISOString(), error: String(e).slice(0, 120) }));
  }
  if (last) break;
  await sleep(every);
}
