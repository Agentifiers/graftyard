// narrow-reads-3's leader memory sampler: every EVERY ms (default 15,000) until DONE_FILE exists, the leader's
// `/stats?summary=1` wasm high-water (`wasmMemMb`), log bytes in memory (`logBytes`), and per learner the messages held
// for its next call (`held`: [addr, n, bytes, peak bytes, calls open]), one JSON line each, so a run's memory climb can be
// matched to held and log bytes, and the leader memory split (`memSplit`: fold rows and caches, log, held, in-flight
// bodies). No reset: the run's own `/stats` reads are untouched.
//   DONE_FILE=path node tests/nr3/lmem.mjs repo:ID >> tests/nr3/raw/lmem-TAG.jsonl
const { member, leaderOf, voterNames, sleep } = await import("../am1/hosted/lib.mjs");
const { existsSync } = await import("node:fs");
const chain = process.argv[2];
const every = Number(process.env.EVERY ?? 15_000);
while (!(process.env.DONE_FILE && existsSync(process.env.DONE_FILE))) {
  try {
    const leader = await leaderOf(chain);
    // Every voter, not only the leader: on the gate 16 A/B the followers jumped first. `site` names the member; the
    // leader's row is the one with site == leader.
    const names = await voterNames(chain).catch(() => ({}));
    await Promise.all([0, 1, 2].map(async (site) => {
      try {
        const v = (await member(chain, site, "/stats?summary=1", undefined, 15_000, names[site])).v ?? {};
        console.log(JSON.stringify({ at: new Date().toISOString(), leader, site, wasmMemMb: v.wasmMemMb, logBytes: v.logBytes, held: v.held, memSplit: v.memSplit }));
      } catch (e) {
        console.log(JSON.stringify({ at: new Date().toISOString(), leader, site, error: String(e).slice(0, 120) }));
      }
    }));
  } catch (e) {
    console.log(JSON.stringify({ at: new Date().toISOString(), error: String(e).slice(0, 120) }));
  }
  await sleep(every);
}
