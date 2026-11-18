// narrow-reads-3's trace saver (operator after gate 16): every EVERY ms (default 60,000) until DONE_FILE exists, each
// voter's test-faults `/trace?since=` rows after the last ones saved from it, one JSON line per call:
// {at, site, leader, rows}. A member whose isolate resets loses its `Stats::trace`, so these keep what came before;
// legs.py merges them into the run's trace (`v<site>` and the leader's `f<site>`).
//   DONE_FILE=path node tests/nr3/tsave.mjs repo:ID > tests/nr3/raw/tsave-TAG.jsonl
// LEARNERS=1 saves each learner's rows too, as {at, learner: address, rows} (F74's call trace, tests/f74/calls.py).
const { member, statuses, voterNames, sleep } = await import("../am1/hosted/lib.mjs");
const { existsSync } = await import("node:fs");
// `at` on Cloudflare's clock (tests/fanout/README.md, "Clocks"); the first line logs the orb's measured offset.
const clk = (await import("../fanout/srvclock.mjs")).serverClock();
await clk.ready;
console.log(JSON.stringify({ at: clk.iso(), clock: clk.offset() }));
const chain = process.argv[2];
const every = Number(process.env.EVERY ?? 60_000);
const since = [0, 0, 0];
const learnerSince = {};
const done = () => process.env.DONE_FILE && existsSync(process.env.DONE_FILE);
for (;;) {
  const last = done();
  try {
    const st = await statuses(chain);
    const leader = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0]?.site ?? null;
    const names = await voterNames(chain).catch(() => ({}));
    for (const site of [0, 1, 2]) {
      try {
        const r = await member(chain, site, `/trace?since=${since[site]}`, undefined, 30_000, names[site]);
        // A tree without the route (before fc8ecce4) answers 404: logged, nothing kept.
        if (!Array.isArray(r.v)) throw new Error(`status ${r.status}: ${JSON.stringify(r.v).slice(0, 80)}`);
        const rows = r.v;
        // A reset member's clock moves on, so `since` stays valid; its new rows are all later.
        if (rows.length) since[site] = rows[rows.length - 1][0];
        console.log(JSON.stringify({ at: clk.iso(), site, leader, rows }));
      } catch (e) {
        console.log(JSON.stringify({ at: clk.iso(), site, error: String(e).slice(0, 120) }));
      }
    }
    if (process.env.LEARNERS) {
      const cfg = st.find((s) => s.role === "leader")?.config?.config?.normal;
      for (const [i, l] of (cfg?.learners ?? []).entries()) {
        const addr = 192 + i;
        try {
          const r = await member(chain, l.site, `/trace?since=${learnerSince[addr] ?? 0}`, undefined, 30_000, l.do_name);
          if (!Array.isArray(r.v)) throw new Error(`status ${r.status}: ${JSON.stringify(r.v).slice(0, 80)}`);
          if (r.v.length) learnerSince[addr] = r.v[r.v.length - 1][0];
          console.log(JSON.stringify({ at: clk.iso(), learner: addr, rows: r.v }));
        } catch (e) {
          console.log(JSON.stringify({ at: clk.iso(), learner: addr, error: String(e).slice(0, 120) }));
        }
      }
    }
  } catch (e) {
    console.log(JSON.stringify({ at: clk.iso(), error: String(e).slice(0, 120) }));
  }
  if (last) break;
  await sleep(every);
}
