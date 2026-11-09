// LF-13 hosted (`build/BUILD-PLAN.md`, AM1-WP4): 1,000 polls held on a follower F, then a fault that leaves F behind
// its leader's next round, then catch-up. The day-6 run blackholed F for 5 s under one leader and never left a gap:
// the leader re-sends from F's matched index (`crates/raft/tests/lf13.rs`, `rounds_lost_to_the_same_leader_...`). So:
//
//   gap      F's member calls fail (`/_bed/fault?site=F`) while lease commits run, and leadership moves to the other
//            follower meanwhile (member `/transfer`); when the fault lifts, the new leader's first round to F starts
//            past F's last entry and F refuses it (`AppendRes { ok: false }`), which the feed marks as a gap.
//   install  F's member calls fail while lease commits run and the leader's archive alarm, held to a small cap
//            (`/bench_seed {"archiveCap"}`, test-faults), archives and truncates past F's last entry; when the fault
//            lifts, F is sent an `InstallSnapshot`. Needs the archive alarm (am1-rest) deployed, and both tiers'
//            snapshots on the leader.
//
// Then: every held poll returned within 1 s of the fault's end with its cursor and no events; new polls to F while it
// is behind answer `lagging` (sampled every 200 ms); and a poll from the held polls' cursor, once F follows again,
// returns every lease event committed meanwhile (none lost).
//
//   node tests/am1/rest/lf13.mjs <gap|install> --repo NAME --chain repo:ID
// Environment as tests/am1/hosted/lib.mjs. Rows: tests/am1/raw/lf13-<mode>.jsonl.

import { parseArgs } from "node:util";
import { api, bedGet, ghs, log, member, row, sleep, statuses } from "../hosted/lib.mjs";

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

/** A poll straight to F's `/feed`, as the edge sends it, with its own token and IP hashes. */
async function poll(site, k, wait, cursor = null) {
  const t0 = Date.now();
  try {
    const r = await member(CHAIN, site, "/feed", { cursor, wait_s: wait, paths: ["lf13/**"], learner_answers: false, token: `lf13-${tag}-${k}`, ip: `lf13ip-${k}` }, (wait + 15) * 1000);
    return { admit: r.v?.admit ?? `status ${r.status}`, events: r.v?.events ?? [], cursor: r.v?.cursor, stopped: !!r.v?.stopped, end: Date.now(), ms: Date.now() - t0 };
  } catch (e) { return { admit: `error ${String(e).slice(0, 50)}`, events: [], end: Date.now(), ms: Date.now() - t0 }; }
}

// Lease commits: an acquire then a release on `lf13/<tag>/<k>/**`, each one feed event with that path.
const app = (await ghs({})).body.token;
const task = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `lf13-${tag}`, summary: "lf13" });
if (task.status !== 201) throw new Error(`task ${task.status}`);
const h = { "x-graftyard-agent-session": task.body.id };
const committed = [];
async function leases(ms) {
  const end = Date.now() + ms;
  let k = 0;
  while (Date.now() < end) {
    const a = await api("POST", `${V1}/leases`, app, { paths: [`lf13/${tag}/${k}/**`], mode: "shared", ttl_seconds: 600 }, h);
    if (a.status === 201) {
      committed.push(`acquired ${a.body.lease.id}`);
      const d = await api("DELETE", `${V1}/leases/${a.body.lease.id}`, app, undefined, h);
      if (d.status === 204) committed.push(`released ${a.body.lease.id}`);
    }
    k++;
  }
}

const st = await feedOf();
const leader = st.find((m) => m.role === "leader")?.site;
const [F, O] = st.filter((m) => m.role === "follower").map((m) => m.site);
if (leader === undefined || F === undefined || O === undefined) throw new Error(`need a leader and two followers: ${JSON.stringify(st)}`);
row(OUT, { step: "start", mode, leader, follower: F, other: O, members: st });
log(`lf13 ${mode}: leader ${leader}, F ${F}, other ${O}`);
await leases(3_000);
const before = (await poll(F, 99_999, 0)).cursor;

// 1,000 held on F, spread over 2 s.
const held = Array.from({ length: 1000 }, (_, k) => sleep(k * 2).then(() => poll(F, k, 25)));
await sleep(4_000);
row(OUT, { step: "held", members: await feedOf() });

let faultMs = 5_000;
const on = await bedGet(`/_bed/fault?site=${F}&for_ms=${mode === "gap" ? 30_000 : 300_000}`);
const fault0 = Date.now();
const during = leases(mode === "gap" ? 8_000 : 60_000);
if (mode === "gap") {
  await sleep(1_000);
  const t = await member(CHAIN, leader, "/transfer", { to: O }, 15_000);
  row(OUT, { step: "transfer", to: O, answer: t.v ?? t.status });
  for (let i = 0; i < 20 && (await statuses(CHAIN)).find((s) => s.site === O)?.role !== "leader"; i++) await sleep(500);
} else {
  // A cap far below the log, so the leader promotes and archives every range above its snapshots, and truncates.
  const cap = await member(CHAIN, leader, "/bench_seed", { archiveCap: { max: 4_096, range: 65_536 } }, 15_000);
  row(OUT, { step: "archiveCap", answer: cap.v ?? cap.status });
  for (let i = 0; i < 120; i++) {
    await sleep(2_000);
    const s = (await member(CHAIN, leader, "/stats", undefined, 15_000)).v ?? {};
    const fs = (await statuses(CHAIN)).find((x) => x.site === F);
    if (s.truncatedTo > (fs?.applied ?? 0) + 1) { row(OUT, { step: "truncated", truncatedTo: s.truncatedTo, fApplied: fs?.applied, archiveSteps: s.archiveSteps }); break; }
  }
}
await during;
faultMs = Date.now() - fault0;
const off = await bedGet("/_bed/fault?off=1");
const lifted = Date.now();
// New polls to F while it is behind, every 200 ms for 10 s.
const probes = [];
for (let i = 0; i < 50; i++) { const p = await poll(F, 50_000 + i, 0); probes.push([Date.now() - lifted, p.admit]); await sleep(200); }
const res = await Promise.all(held);
// Every held poll's return, against the fault's end.
const ret = res.map((r) => r.end - lifted);
const afterLift = ret.filter((x) => x >= 0);
// No event lost: from the cursor before the fault, F (now following) and the leader list the same lease events.
await sleep(3_000);
const cur = res.find((r) => r.cursor)?.cursor ?? before;
const fromF = await poll(F, 77_777, 0, before);
const now = (await statuses(CHAIN)).find((s) => s.role === "leader")?.site;
// The reference: the voter that is neither F nor the leader now, from the same cursor.
const ref = [leader, O].find((x) => x !== now && x !== F);
const fromRef = ref === undefined ? null : await poll(ref, 77_778, 0, before);
const ids = (evs) => evs.map((e) => e[2]?.Lease).filter(Boolean).map((l) => `${l.change} ${l.lease?.id?.join?.(",") ?? JSON.stringify(l.lease?.id)}`);
const fIds = ids(fromF.events), rIds = ids(fromRef?.events ?? []);
const v = {
  mode, fault: { status: on.status, ms: faultMs, off: off.status }, leaderAfter: now,
  held: { n: res.length, admits: count(res.map((r) => r.admit)), stopped: res.filter((r) => r.stopped).length,
    withEvents: res.filter((r) => r.events.length > 0).length, withCursor: res.filter((r) => r.cursor).length,
    returnedBeforeLift: ret.filter((x) => x < 0).length, afterLiftMs: { p50: pct(afterLift, 0.5), p99: pct(afterLift, 0.99), max: afterLift.length ? Math.max(...afterLift) : null } },
  probes: { lagging: probes.filter((p) => p[1] === "lagging").length, firstHold: probes.find((p) => p[1] === "hold")?.[0] ?? null, seq: probes.slice(0, 12) },
  // F's ring after a gap holds every entry it applied; after an install it starts above `through`, so an older
  // cursor may get `gone` with the oldest it can serve (the design's ring rule), and that is recorded as such.
  events: { committed: committed.length, fromF: fIds.length, fAdmit: fromF.admit, reference: ref, fromRef: rIds.length,
    missingOnF: rIds.filter((x) => !fIds.includes(x)).length, heldCursor: cur },
  members: await feedOf(),
};
row(OUT, v);
log(JSON.stringify(v).slice(0, 3000));
