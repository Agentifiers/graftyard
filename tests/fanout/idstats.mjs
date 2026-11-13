// After a smoke or row: each voter's `/stats` peer calls for the identity groups that stepped down, so a missed
// CALL_DEADLINE can be read from the leader's side (kind, call, entries, bytes, ms) instead of from Workers Logs walls,
// which count a follower's 3.1 s ticker after its reply (LM-1 round 8).
//   node tests/fanout/idstats.mjs tokens:2490 names:708 ... > tests/fanout/raw/idstats-TAG.json
import { member } from "../am1/hosted/lib.mjs";

const out = {};
for (const chain of process.argv.slice(2)) {
  out[chain] = [];
  for (const site of [0, 1, 2]) {
    const r = await member(chain, site, "/stats", undefined, 15_000).catch((e) => ({ v: { err: String(e).slice(0, 120) } }));
    const v = r.v ?? {};
    out[chain].push({
      site, role: v.role, failedCalls: v.failedCalls, err: v.err,
      peers: Object.fromEntries(Object.entries(v.peers ?? {}).map(([p, x]) => [p, {
        ok: x.ok, failed: x.failed, late: x.late, lateMsMax: x.lateMsMax, lateFailed: x.lateFailed, errors: x.errors,
        appends: (x.appends ?? []).filter((a) => a.ms >= 500),
      }])),
      appendMs: v.appendMs, confirmMs: v.confirmMs?.length ? [...v.confirmMs].sort((a, b) => a - b).at(-1) : null,
    });
  }
}
console.log(JSON.stringify(out));
