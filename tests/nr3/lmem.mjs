// narrow-reads-3's leader memory sampler: every EVERY ms (default 15,000) until DONE_FILE exists, the leader's
// `/stats?summary=1` wasm high-water (`wasmMemMb`), log bytes in memory (`logBytes`), and per learner the messages held
// for its next call (`held`: [addr, n, bytes, peak bytes, calls open]), one JSON line each, so a run's memory climb can be
// matched to held and log bytes, and the leader memory split (`memSplit`: fold rows and caches, log, held, in-flight
// bodies). No reset: the run's own `/stats` reads are untouched.
//   DONE_FILE=path node tests/nr3/lmem.mjs repo:ID >> tests/nr3/raw/lmem-TAG.jsonl
const { member, leaderOf, sleep } = await import("../am1/hosted/lib.mjs");
const { existsSync } = await import("node:fs");
const chain = process.argv[2];
const every = Number(process.env.EVERY ?? 15_000);
while (!(process.env.DONE_FILE && existsSync(process.env.DONE_FILE))) {
  try {
    const leader = await leaderOf(chain);
    const v = (await member(chain, leader, "/stats?summary=1", undefined, 15_000)).v ?? {};
    console.log(JSON.stringify({ at: new Date().toISOString(), leader, wasmMemMb: v.wasmMemMb, logBytes: v.logBytes, held: v.held, memSplit: v.memSplit }));
  } catch (e) {
    console.log(JSON.stringify({ at: new Date().toISOString(), error: String(e).slice(0, 120) }));
  }
  await sleep(every);
}
