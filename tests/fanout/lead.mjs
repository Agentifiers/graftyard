// lead.mjs CHAIN -> one line: each voter's role, term, leader, vote, applied index and promise, for watching a group
// during a row (tests/fanout/leadwatch.sh). Output passes through lib.mjs's scan.
import { member, scan } from "/home/user/workspace/repo/.worktrees/fanout/tests/am1/hosted/lib.mjs";
const chain = process.argv[2];
const out = [];
for (const s of [0, 1, 2]) {
  const r = await member(chain, s, "/status", undefined, 15_000).catch((e) => ({ v: { error: String(e).slice(0, 80) } }));
  const v = r.v ?? {};
  out.push({ s, role: v.role, term: v.term, leader: v.leader, voted: v.votedFor, applied: v.applied, promiseInMs: v.promiseUntil && v.now ? v.promiseUntil - v.now : v.promiseUntil, stalledAgoMs: v.stalledSince && v.now ? v.now - v.stalledSince : null, boots: v.boots, err: v.error });
}
console.log(scan(new Date().toISOString() + " " + JSON.stringify(out)));
