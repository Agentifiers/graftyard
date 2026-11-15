// LM-1 round 20: B0-34's promotion drain under stored-fetch's pack-bytes trigger, timed on its own before the cold warm.
// Wakes the group (an edge read), then every 30 s reads the leader's committed `ranges` (`promoted`, `archived`) and
// `/status` (`commit`, `last`) and the archive notes in `/stats` `alarms`, one JSON line each. Promote-ahead stops once
// the pack bytes above the last promotion are under LOG_MAX_BYTES / PROMOTE_AHEAD, so the drain is done when the last
// promotion has moved and then not moved for STILL_S (default 180). The first promotion waits for the leader site's
// shard to catch up, a cold rehydrate of thin packs, so two numbers: `firstS` (to the first promotion, the shard's
// warm) and `drainS` (first to last promotion). Past `limit` minutes in all, exit 2. A stop short of the tail is exit 3,
// except that a 503 warming in the first WARM_GRACE_S (2700) is the shard's cold rehydrate, not a stall. Read only.
//   node tests/fanout/b034drain.mjs <repo:ID> <owner/name> [limit minutes 90]
import { api, leaderOf, member, sleep, tokens } from "../am1/hosted/lib.mjs";
const [chain, repo, limitMin = "90"] = process.argv.slice(2);
const t0 = Date.now();
const lim = Number(limitMin) * 60;
const s = () => +((Date.now() - t0) / 1000).toFixed(1);
const out = (v) => console.log(JSON.stringify({ at: new Date().toISOString(), s: s(), ...v }));
await api("GET", `/api/v3/repos/${repo}/branches/main`, tokens.admin).catch(() => {});
let target = null, first = null, last = null, firstS = null, lastMoveS = null, seen = new Set();
const grace = Number(process.env.WARM_GRACE_S ?? 2700);
const still = Number(process.env.STILL_S ?? 180);
for (;;) {
  try {
    const leader = await leaderOf(chain);
    const st = (await member(chain, leader, "/status", undefined, 15_000)).v ?? {};
    const r = (await member(chain, leader, "/read", { q: { ranges: true } }, 20_000)).v?.value ?? {};
    const al = ((await member(chain, leader, "/stats?summary=1", undefined, 15_000)).v?.alarms ?? []).filter((a) => !seen.has(a));
    al.forEach((a) => seen.add(a));
    target ??= st.commit;
    const to = r.promoted?.to ?? 0;
    first ??= to;
    out({ kind: "drain", leader, term: st.term, commit: st.commit, last: st.last, promotedTo: to, promotedFrom: r.promoted?.from,
          pack: r.promoted?.pack, archivedTo: r.archived?.to ?? null, target, alarms: al.slice(-8) });
    if (to !== last) { if (last !== null) { lastMoveS = s(); firstS ??= lastMoveS; } last = to; }
    // Drained only when the promotions reach the pushed tail; a promotion that stops short of it for STILL_S is a
    // stall (round 20: entry 18's one 53 MB pack over PROMOTE_MAX stopped every run at 17 of 648), exit 3.
    // Round 22: in the first WARM_GRACE_S (45 min), promotions answered 503 warming are the leader site's shard still
    // rehydrating cold, not a stall. Those 503s reach no member alarm (round 21's 89 samples read none), so the grace
    // is by time: no stall ends the drain before it.
    const drained = to >= target - 8;
    if (lastMoveS !== null && s() - lastMoveS >= still && (drained || s() >= grace)) {
      out({ kind: "summary", drained, stalled: !drained, firstS, drainS: +(lastMoveS - firstS).toFixed(1), promotedFrom0: first,
            promotedTo: to, target, alarms: al.slice(-3) });
      process.exit(drained ? 0 : 3);
    }
  } catch (e) { out({ kind: "error", e: String(e).slice(0, 160) }); }
  // Round 22: one limit on the whole drain (90 min in b034.sh's DRAIN_MIN), first promotion and the rest together.
  if (s() > lim) {
    out({ kind: "summary", drained: false, firstS, lastMoveS, promotedFrom0: first, promotedTo: last, limitMin: Number(limitMin) });
    process.exit(2);
  }
  await sleep(30_000);
}
