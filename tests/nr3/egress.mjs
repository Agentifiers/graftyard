// narrow-reads-3, learners pipelined: the leader's calls to each member in a run (`/stats` `peers`: calls ended,
// encoded bytes, ok, late, failed), so its egress per voter and per learner shows beside the run's CPU.
//   node tests/nr3/egress.mjs repo:ID
const { member, leaderOf } = await import("../am1/hosted/lib.mjs");
const chain = process.argv[2];
const leader = await leaderOf(chain);
const v = (await member(chain, leader, "/stats", undefined, 30000)).v ?? {};
const out = {};
let bytes = 0, calls = 0;
for (const [to, p] of Object.entries(v.peers ?? {})) {
  const failed = Object.values(p.failed ?? {}).reduce((a, b) => a + b, 0);
  out[to] = { calls: p.sent, mb: +((p.sentBytes ?? 0) / 1e6).toFixed(1), ok: p.ok, late: p.late, failed };
  bytes += p.sentBytes ?? 0;
  calls += p.sent ?? 0;
}
console.log("egress", JSON.stringify({ leader, calls, mb: +(bytes / 1e6).toFixed(1), peers: out }));
