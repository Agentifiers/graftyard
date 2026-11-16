// L0-12's drain wait between the replay and the stall: polls every member of the group (voters and learners) through
// `/status` until none still serves the replay's requests: no feed poll held, no read in flight, and nothing queued
// at the leader's admission. On m3fr9 the stall's first task create, 4 s after the replay ended, got 503 "every member
// asked shed the read: overloaded" (`tests/am1/results/lm1-rows-m3f.md`, Rows m3fr9, L0-12). It waits; it never
// retries a request. One JSON line per round; exit 0 once drained, 2 past `limit`.
//   node tests/fanout/drain.mjs <repo id> [limit seconds 120]
import { log, member, statuses } from "../am1/hosted/lib.mjs";

const [id, limitS = "120"] = process.argv.slice(2);
const chain = `repo:${id}`;
const t0 = Date.now();
const until = t0 + Number(limitS) * 1000;
for (;;) {
  const voters = await statuses(chain);
  const cfg = voters.find((s) => s.config)?.config?.config?.normal;
  const learners = await Promise.all((cfg?.learners ?? []).map((l) =>
    member(chain, l.site, "/status", undefined, 10_000, l.do_name).then((r) => ({ site: l.site, learner: true, ...r.v }), (e) => ({ site: l.site, learner: true, error: String(e).slice(0, 80) }))));
  const busy = [...voters, ...learners].map((s) => ({
    site: s.site, learner: !!s.learner, role: s.role, error: s.error,
    held: s.feed?.held ?? 0, reads: s.readsInFlight, queued: s.admission ? s.admission.queued + s.admission.agentQueued : 0,
  }));
  const open = busy.filter((b) => b.error || b.held > 0 || b.reads > 0 || b.queued > 0);
  console.log(JSON.stringify({ at: new Date().toISOString(), ms: Date.now() - t0, open }));
  if (open.length === 0) {
    log(`drained after ${((Date.now() - t0) / 1000).toFixed(1)} s (${busy.length} members)`);
    process.exit(0);
  }
  if (Date.now() > until) {
    log(`NOT drained after ${limitS} s: ${JSON.stringify(open).slice(0, 400)}`);
    process.exit(2);
  }
  await new Promise((d) => setTimeout(d, 5_000));
}
