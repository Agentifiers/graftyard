// LF-13 hosted (`build/BUILD-PLAN.md`, AM1-WP4): 1,000 polls held on a follower F, then a fault that leaves F behind
// its leader's next round, then catch-up. The day-6 run blackholed F for 5 s under one leader and never left a gap:
// the leader re-sends from F's matched index (`crates/raft/tests/lf13.rs`, `rounds_lost_to_the_same_leader_...`). So:
//
//   gap      F is severed (member `/sever`, test-faults: every call to and from that one member fails, set and lifted
//            in the object at once) while lease commits run, and leadership moves to the other follower O meanwhile
//            (member `/transfer`) once O holds an entry F lacks; when the sever lifts, the new leader's first round to
//            F starts past F's last entry and F refuses it (`AppendRes { ok: false }`), which the feed marks as a gap.
//   install  F is severed while lease commits run and the leader's archive alarm, held to a small cap
//            (`/bench_seed {"archiveCap"}`, test-faults), archives and truncates past F's last entry; when the fault
//            lifts, F is sent an `InstallSnapshot`. Needs the archive alarm (am1-rest) deployed, and both tiers'
//            snapshots on the leader.
//
// Then: every held poll returned within 1 s with its cursor and no events, timed on F's clock both from the lift (the
// `/sever` answer's `now`) and from F's stop (its feed's `lastStop.at`), since A26 asks which one the row means; new
// polls to F while it is behind answer `lagging` (sampled every 200 ms); and a poll from the held polls' cursor, once
// F follows again, returns every lease event committed meanwhile (none lost).
//
// The lease commits are m3-member's (a PAT with the org's base Write): a Shared lease needs no task, so the run needs
// no installation token and the install no `graft-ops`.
//
//   node tests/am1/rest/lf13.mjs <gap|install> --repo NAME --chain repo:ID
// Environment as tests/am1/hosted/lib.mjs. Rows: tests/am1/raw/lf13-<mode>.jsonl.

import { parseArgs } from "node:util";
import { api, log, member, row, sleep, statuses, tokens } from "../hosted/lib.mjs";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: { repo: { type: "string" }, chain: { type: "string" } } });
const mode = positionals[0];
if (!["gap", "install"].includes(mode) || !opt.repo || !opt.chain) throw new Error("usage: lf13.mjs <gap|install> --repo NAME --chain repo:ID");
const R = `m3org/${opt.repo}`;
const CHAIN = opt.chain;
const V1 = `/api/graftyard/v1/repos/${R}`;
const OUT = new URL(`../raw/lf13-${mode}.jsonl`, import.meta.url).pathname;
const tag = Date.now().toString(36);
const pct = (xs, p) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const count = (xs) => xs.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {});
const feedOf = async () => (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, feed: s.feed, applied: s.applied }));

/** A poll straight to F's `/feed`, as the edge sends it, with its own token and IP hashes. The held polls ask for
 *  `lf13-held/**`, which no write of this driver touches, so only F stopping (or `wait` ending) returns them; the
 *  lease commits that make the fault are `lf13/**` events. */
async function poll(site, k, wait, cursor = null, paths = ["lf13/**"]) {
  const t0 = Date.now();
  try {
    const r = await member(CHAIN, site, "/feed", { cursor, wait_s: wait, paths, learner_answers: false, token: `lf13-${tag}-${k}`, ip: `lf13ip-${k}` }, (wait + 15) * 1000);
    return { admit: r.v?.admit ?? `status ${r.status}`, events: r.v?.events ?? [], cursor: r.v?.cursor, oldest: r.v?.oldest, stopped: !!r.v?.stopped,
      stoppedAt: r.v?.stoppedAt ?? null, answeredAt: r.v?.answeredAt ?? r.v?.at ?? null, end: Date.now(), ms: Date.now() - t0 };
  } catch (e) { return { admit: `error ${String(e).slice(0, 50)}`, events: [], end: Date.now(), ms: Date.now() - t0 }; }
}

// Lease commits: an acquire then a release on `lf13/<tag>/<k>/**`, each one feed event with that path.
const app = tokens.member;
const h = {};
const committed = [];
const refusals = [];
let leasesUntil = 0;
async function leases(ms) {
  const end = Date.now() + ms;
  leasesUntil = end;
  let k = 0;
  while (Date.now() < Math.min(end, leasesUntil)) {
    const a = await api("POST", `${V1}/leases`, app, { paths: [`lf13/${tag}/${k}/**`], mode: "shared", ttl_seconds: 600 }, h);
    if (a.status === 201) {
      committed.push(`acquired ${a.body.lease.id}`);
      const d = await api("DELETE", `${V1}/leases/${a.body.lease.id}`, app, undefined, h);
      if (d.status === 204) committed.push(`released ${a.body.lease.id}`);
      else if (refusals.length < 5) refusals.push(`release ${d.status}`);
    } else if (refusals.length < 5) refusals.push(`acquire ${a.status} ${JSON.stringify(a.body?.message ?? "").slice(0, 80)}`);
    k++;
  }
}

// An idle group has no leader (its members sleep); a commit through the edge wakes it, then one leader is awaited.
// A leader off the group's home (site 0, the `Tri` record's) hands off to it 10 s after it starts (`maybe_hand_off`),
// which after the last run's transfer lands mid-run and makes F the leader (`notServing` to every held poll). So the
// run starts with the home leading: moved there now, as the hand-off would, and awaited.
const HOME = 0;
await leases(1_000);
let st = await feedOf();
for (let i = 0; i < 40; i++) {
  const ls = st.filter((m) => m.role === "leader");
  if (ls.length === 1 && ls[0].site === HOME) break;
  if (ls.length === 1 && i % 10 === 0) row(OUT, { step: "home", from: ls[0].site, answer: (await member(CHAIN, ls[0].site, "/transfer", { to: HOME }, 15_000).catch((e) => ({ status: String(e).slice(0, 80) }))).v ?? null });
  await sleep(1_000);
  st = await feedOf();
}
// And every member follows (no install or gap left from an earlier run), up to 3 minutes.
for (let i = 0; i < 90 && !st.every((m) => m.feed?.following !== false || m.role === "leader"); i++) { await sleep(2_000); st = await feedOf(); }
const leader = st.find((m) => m.role === "leader")?.site;
const [F, O] = st.filter((m) => m.role === "follower").map((m) => m.site);
if (leader === undefined || F === undefined || O === undefined) throw new Error(`need a leader and two followers: ${JSON.stringify(st)}`);
row(OUT, { step: "start", mode, leader, follower: F, other: O, members: st });
log(`lf13 ${mode}: leader ${leader}, F ${F}, other ${O}`);
if (mode === "install") {
  // The archive alarm sums the log at most once a minute while it finds nothing to do, and a held poll waits at most
  // 25 s, so the cap is set first and the archiver kept busy (each run that archives re-arms in 200 ms) by 40 s of
  // commits before any poll is held; the fault then truncates past F within seconds and lifts as soon as it has.
  // The archive keeps about `max` bytes of log, about 5 lease entries at 1,024 (at 4,096, about 20, which took over
  // 20 s at about one commit a second, past the held polls' 25 s: graft-m3c's first install runs).
  const cap = await member(CHAIN, leader, "/bench_seed", { archiveCap: { max: 1_024, range: 65_536 } }, 15_000);
  row(OUT, { step: "archiveCap", answer: cap.v ?? cap.status });
  await leases(40_000);
  row(OUT, { step: "warm", stats: (({ truncatedTo, archiveSteps }) => ({ truncatedTo, archiveSteps }))((await member(CHAIN, leader, "/stats", undefined, 15_000)).v ?? {}) });
}
await leases(3_000);
const before = (await poll(F, 99_999, 0)).cursor;

// HELD (default 1,000) held on F, spread over 2 s. At FEED_HELD_MAX (1,000) a new poll is answered `full` until they
// return, which hides `lagging` from the probes (fanout, LF-13 round 3); HELD=999 leaves one slot for them.
const HELD = Number(process.env.HELD ?? 1000);
const held = Array.from({ length: HELD }, (_, k) => sleep(k * 2).then(() => poll(F, k, 25, null, ["lf13-held/**"])));
await sleep(2_500);
const atHold = await feedOf();
row(OUT, { step: "held", members: atHold });
if (atHold.find((m) => m.site === F)?.role !== "follower") throw new Error(`F (${F}) is not a follower once the polls are held`);

let faultMs = 5_000;
// The sever outlives the run's own lift by far, so a driver that dies leaves it on for at most 60 s.
const on = await member(CHAIN, F, "/sever", { for_ms: 60_000 }, 15_000);
const fault0 = Date.now();
row(OUT, { step: "sever", answer: on.v ?? on.status });
// Install mode runs two lease loops, so the log passes F's last entry by the cap's few entries sooner.
const during = mode === "gap" ? leases(15_000) : Promise.all([leases(20_000), leases(20_000)]);
if (mode === "gap") {
  // A transfer before O holds an entry F lacks would elect O at an entry F already holds, and O's first round would
  // match (`am1-runs`, `lm1-rerun`: O level with F in 8 of 9 runs under the site fault, which takes up to a second
  // per isolate to start). The sever stops F at once, so wait until F's `last` has stood still while O's is past it:
  // only then does O's first round, from its own last entry, start past F's.
  const w0 = Date.now();
  let fLast = -1, fSince = Date.now(), gapSeen = null, oLast = -1;
  while (Date.now() - w0 < 11_000) {
    await sleep(300);
    const s = await statuses(CHAIN);
    const fl = s.find((x) => x.site === F)?.last ?? -1;
    oLast = s.find((x) => x.site === O)?.last ?? -1;
    if (fl !== fLast) { fLast = fl; fSince = Date.now(); }
    if (Date.now() - fSince >= 600 && oLast > fl) { gapSeen = { fLast: fl, oLast, ms: Date.now() - fault0 }; break; }
  }
  row(OUT, { step: "fStopped", gapSeen, fLast, oLast });
  // The fault stays on until O leads and has committed entries F lacks, so O's first round to F starts past F's
  // last entry (`crates/raft/tests/lf13.rs`, `a_new_leader_s_first_round_leaves_a_gap_on_the_lagging_follower`). A
  // transfer the leader drops (it waits out F's missing ack, then abandons) is re-sent, up to 5 times.
  // A held poll waits at most FEED_WAIT_MAX (25 s), and F refuses new polls while faulted, so the fault must lift
  // within about 20 s of the polls: the transfer is retried until 14 s into the fault.
  let moved = false;
  for (let tries = 0; tries < 5 && !moved && Date.now() - fault0 < 14_000; tries++) {
    const t = await member(CHAIN, leader, "/transfer", { to: O }, 15_000).catch((e) => ({ status: String(e).slice(0, 80) }));
    row(OUT, { step: "transfer", to: O, try: tries, answer: t.v ?? t.status });
    for (let i = 0; i < 12 && !moved; i++) { await sleep(500); moved = (await statuses(CHAIN)).find((s) => s.site === O)?.role === "leader"; }
  }
  // O commits past F: lease commits through the edge, which finds the new leader.
  const fAt = (await statuses(CHAIN)).find((s) => s.site === F)?.applied ?? 0;
  await sleep(2_000);
  leasesUntil = 0;
  const st2 = await statuses(CHAIN);
  row(OUT, { step: "pastF", moved, fApplied: fAt, leaderCommit: st2.find((s) => s.role === "leader")?.commit, members: st2.map((m) => [m.site, m.role, m.term, m.applied, m.last]) });
} else {
  // The cap (set above) is far below the log, so the leader archives each range as it fills, and truncates past F. A
  // held poll waits at most 25 s from its admission, about 3 s before the sever, so the sever lifts by 18 s in.
  let truncated = null;
  for (let i = 0; i < 34 && !truncated && Date.now() - fault0 < 17_500; i++) {
    await sleep(500);
    const s = (await member(CHAIN, leader, "/stats", undefined, 15_000)).v ?? {};
    const fs = (await statuses(CHAIN)).find((x) => x.site === F);
    if (s.truncatedTo > (fs?.applied ?? 0) + 1) truncated = { truncatedTo: s.truncatedTo, fApplied: fs?.applied, archiveSteps: s.archiveSteps, ms: Date.now() - fault0 };
  }
  row(OUT, { step: "truncated", truncated });
  leasesUntil = 0;
  // Under the small cap each `Archived` entry makes the next archive due, so the leader archives about once a second
  // with no commits, its install source moves under F's load, and F's finished load answers a newer source and is
  // refused ("the loaded fold is not at through", graft-m3c's second install runs). The cap goes once it has
  // truncated past F; the leader then keeps the source the install loads.
  row(OUT, { step: "uncap", answer: (await member(CHAIN, leader, "/bench_seed", { archiveCap: {} }, 15_000)).v ?? null });
}
await during;
faultMs = Date.now() - fault0;
const off = await member(CHAIN, F, "/sever", { for_ms: 0 }, 15_000);
const lifted = Date.now();
const liftAt = off.v?.now ?? null;
// New polls to F while it is behind, every 200 ms for 10 s.
const probes = [];
// Each probe also carries its answer time on F's clock less the lift's, to compare with F's stop on one clock.
for (let i = 0; i < 50; i++) { const p = await poll(F, 50_000 + i, 0); probes.push([Date.now() - lifted, p.admit, p.answeredAt == null || liftAt == null ? null : p.answeredAt - liftAt]); await sleep(200); }
const res = await Promise.all(held);
// Every held poll's return, against the lift: as the driver saw it (`end`), and on F's clock (`answeredAt` against
// the `/sever` answer's `now`). And against F's stop, on F's clock: the poll's own `stoppedAt`, or F's `lastStop`.
const ret = res.map((r) => r.end - lifted);
const afterLift = ret.filter((x) => x >= 0);
const fFeed = (await member(CHAIN, F, "/status", undefined, 15_000)).v?.feed ?? {};
const stopAt = fFeed.lastStop?.at ?? null;
const onF = res.filter((r) => r.answeredAt != null);
const fromLift = onF.map((r) => r.answeredAt - liftAt);
const fromStop = onF.map((r) => r.answeredAt - (r.stoppedAt ?? stopAt));
const reading = (xs) => ({ n: xs.length, before: xs.filter((x) => x < 0).length, within1s: xs.filter((x) => x >= 0 && x <= 1000).length,
  over1s: xs.filter((x) => x > 1000).length, p50: pct(xs, 0.5), p99: pct(xs, 0.99), max: xs.length ? Math.max(...xs) : null, min: xs.length ? Math.min(...xs) : null });
// No event lost: from the cursor before the fault, F (now following) and the leader list the same lease events.
// F is read once it serves again (not `lagging`), up to 60 s, so a slow catch-up is not counted as a loss.
await sleep(3_000);
const cur = res.find((r) => r.cursor)?.cursor ?? before;
let fromF = await poll(F, 77_777, 0, before);
const caught0 = Date.now();
while (fromF.admit === "lagging" && Date.now() - caught0 < 60_000) { await sleep(2_000); fromF = await poll(F, 77_777, 0, before); }
const fServedAfterMs = Date.now() - lifted;
const now = (await statuses(CHAIN)).find((s) => s.role === "leader")?.site;
// The reference: the voter that is neither F nor the leader now, from the same cursor.
const ref = [leader, O].find((x) => x !== now && x !== F);
const fromRef = ref === undefined ? null : await poll(ref, 77_778, 0, before);
// Each list is cut at the smaller of the two members' cursors, so an entry one member applied and the other not yet
// is not counted as missing or extra (graft-m3c's first batches: the reference trailed F by one entry).
const at = (c) => (c ? Number(Buffer.from(c, "base64url").readBigUInt64BE(9)) : Infinity);
let upTo = Infinity;
const ids = (evs) => evs.filter((e) => e[0] <= upTo).map((e) => e[2]?.Lease).filter(Boolean).map((l) => `${l.change} ${l.lease?.id?.join?.(",") ?? JSON.stringify(l.lease?.id)}`);
upTo = Math.min(at(fromF.cursor), at(fromRef?.cursor));
let fIds = ids(fromF.events), rIds = ids(fromRef?.events ?? []);
// After an install F's ring starts above the snapshot, so the pre-fault cursor is `gone` with the oldest it serves
// (the design's ring rule): a client re-reads from there. No event is lost if F and the reference list the same
// lease events from that oldest cursor on.
let goneFrom = null;
if (fromF.admit === "gone" && fromF.oldest && ref !== undefined) {
  const [f2, r2] = await Promise.all([poll(F, 77_779, 0, fromF.oldest), poll(ref, 77_780, 0, fromF.oldest)]);
  upTo = Math.min(at(f2.cursor), at(r2.cursor));
  goneFrom = { oldest: fromF.oldest, upTo, fAdmit: f2.admit, refAdmit: r2.admit, fromF: ids(f2.events).length, fromRef: ids(r2.events).length };
  fIds = ids(f2.events); rIds = ids(r2.events);
}
const v = {
  mode, fault: { status: on.status, ms: faultMs, off: off.status, liftAt, stopAt, stopToLift: stopAt != null && liftAt != null ? stopAt - liftAt : null, lastStop: fFeed.lastStop ?? null, lastResume: fFeed.lastResume ?? null }, leaderAfter: now, refusals,
  // A26's two readings, on F's clock: from the lift, and from F's stop.
  fromLift: reading(fromLift), fromStop: reading(fromStop),
  held: { n: res.length, admits: count(res.map((r) => r.admit)), stopped: res.filter((r) => r.stopped).length,
    withEvents: res.filter((r) => r.events.length > 0).length, withCursor: res.filter((r) => r.cursor).length,
    returnedBeforeLift: ret.filter((x) => x < 0).length, within1s: ret.filter((x) => x >= 0 && x <= 1000).length,
    noEvents: res.filter((r) => r.events.length === 0).length, afterLiftMs: { p50: pct(afterLift, 0.5), p99: pct(afterLift, 0.99), max: afterLift.length ? Math.max(...afterLift) : null } },
  // Probes F admitted after the lift but before its stop (F's clock): F did not yet know it was behind.
  probes: { heldBeforeStop: stopAt == null ? null : probes.filter((p) => p[1] === "hold" && p[2] != null && p[2] < stopAt - liftAt).length,
    lagging: probes.filter((p) => p[1] === "lagging").length, firstHold: probes.find((p) => p[1] === "hold")?.[0] ?? null, seq: probes.slice(0, 12) },
  // F's ring after a gap holds every entry it applied; after an install it starts above `through`, so an older
  // cursor may get `gone` with the oldest it can serve (the design's ring rule), and that is recorded as such.
  events: { committed: committed.length, fromF: fIds.length, fAdmit: fromF.admit, fServedAfterMs, reference: ref, fromRef: rIds.length,
    missingOnF: rIds.filter((x) => !fIds.includes(x)).length, extraOnF: fIds.filter((x) => !rIds.includes(x)).length, goneFrom, heldCursor: cur },
  members: await feedOf(),
};
row(OUT, v);
log(JSON.stringify(v).slice(0, 3000));
