// Prints the M0-WP9 gate quantities from one run's summary (results/raw/wp9-<row>-<run>.summary.json).
//
//   node run/wp9-report.ts b0-29 <run>

import { readFileSync } from "node:fs";

const [row, run] = process.argv.slice(2);
const ROOT = new URL("../", import.meta.url).pathname;
const s = JSON.parse(readFileSync(`${ROOT}results/raw/wp9-${row}-${run}.summary.json`, "utf8"));
const out: Record<string, unknown> = {
  row, run, seconds: s.seconds, clientsDone: `${s.clientsDone}/${s.clients}`, offeredPerS: s.offeredPerS,
  commitP99: Object.fromEntries(Object.entries(s.commitMs).map(([k, v]: [string, any]) => [k, `${v.p50}/${v.p99}/${v.max} (n ${v.n})`])),
  merges: { n: s.merges.n, ok: s.merges.ok, stale: s.merges.stale, clientP99: s.merges.clientMs.p99, clientMax: s.merges.clientMs.max, leaderP99: s.merges.leaderCommitMs.p99, waited: s.merges.waited },
  pushes: { ok: s.pushesOk, stale: s.stalePushes, refused: s.pushesRefused, error: s.pushesError },
  leaderWindow: s.leaderWindow, leaderReqPerS: s.leader?.reqs?.perS, leaderReqs: s.leader?.reqs?.byMethod, feedOnLeader: s.feed.onLeader, feed: s.feed.polls, feedPerS: s.feed.pollsPerS,
  leaderChanges: s.leaderChanges, suspects: s.suspects, admittedPerS: s.admittedPerS, refusalShapes: s.refusalShapes, badShapes: s.badShapes,
  queuedPeak: s.leader && { cmds: s.leader.agentQueuedPeak, bytes: s.leader.agentBytesPeak, staged: s.leader.staged, demoted: s.leader.demoted, queueRefused: s.leader.queueRefused, capacityAtEnd: Math.round(s.leader.capacityAtEnd) },
  entries: s.entries, reads: Object.fromEntries(Object.entries(s.reads).map(([k, v]: [string, any]) => [k, `${v.ok} ok, ${v.failed} failed, p99 ${v.ms.p99}`])),
  confirmMs: s.confirmMs, events: s.memberEvents, errors: s.errors, firstErr: s.firstErr, clientFlightMax: s.clientFlightMax, shed: s.shed, lateSlots: s.lateSlots,
  burst: s.burst, fastUser: s.fastUser,
};
console.log(JSON.stringify(out, null, 1));
