// M0-WP12 row drivers (build/BUILD-PLAN.md, M0-WP12). Each row runs as named phases; a phase's raw file is its checkpoint.

import { a1, call, commitDist, dist, log, results, setup, sleep, spec, start, watch, pct, type Status } from "./wp12-lib.ts";
import type { Ctx } from "./wp12.ts";
import { AGENT_KINDS, KINDS, RACES, VICTIM, APP, INST, changedPath, ref, scope, seed, session, victimWho, who, grunTask, type Kind } from "../src/wp12/fleet.ts";
import { placeLearners, type Lease, type Who, type XCmd, type XEv, type XOut } from "../src/wp12/lease.ts";

type Dump = { site: string; applied: number; at: number; lastAt: number; leases: Lease[]; tasks: Record<string, string>; extends: number[][]; error?: string };
const leases = (group: string) => call<Dump[]>("/leases", { group }, 60_000);
const faults = (group: string, plan: unknown) => call("/faults", { group, plan });
const rulesOf = (cs: any[]) => { const r: Record<string, number> = {}; for (const c of cs) for (const [k, n] of Object.entries((c.xrules ?? {}) as Record<string, number>)) r[k] = (r[k] ?? 0) + n; return r; };
const feedOf = (cs: any[]) => { const r: Record<string, number> = {}; for (const c of cs) for (const [k, n] of Object.entries((c.feed ?? {}) as Record<string, number>)) r[k] = (r[k] ?? 0) + n; return r; };
const pushRows = (cs: any[]) => cs.flatMap((c) => (c.rows ?? []) as number[][]).filter((r) => r[1] === 0);

/** Kills the leader `n` times, `gap` ms apart, recording who led before and after and how long the group had none. */
async function kills(group: string, n: number, gap: number, first: number): Promise<unknown[]> {
  const out: unknown[] = [];
  await sleep(first);
  for (let i = 0; i < n; i++) {
    const before = await call<Status[]>("/status", { group });
    const leader = before.find((s) => s.role === "leader")?.site;
    const t = Date.now();
    // A machine loss: the leader's object resets, then every call to or from its site fails for 15 s, so the followers'
    // promises lapse and one of them is elected, rather than the reset leader resuming its term after one round. The
    // kill goes first, because the fault would also fail the kill call itself.
    let led: string | undefined, after: Status[] = [];
    if (leader) {
      const killed = await call("/kill", { group, site: leader }).catch((e) => String(e));
      await faults(group, { rules: [{ target: "call", mode: "error", rate: 1, re: `^(edge>${leader}:|${leader}>|[abcl0-9]*>${leader}:)` }], from: Date.now() - 2_000, until: Date.now() + 15_000 });
      log(`kill ${i}: ${leader} answered ${JSON.stringify(killed).slice(0, 60)}`);
      for (let k = 0; k < 120; k++) {
        await sleep(500);
        after = await call<Status[]>("/status", { group }).catch(() => []);
        led = after.find((s) => s.role === "leader" && s.site !== leader)?.site ?? (Date.now() - t > 16_000 ? after.find((s) => s.role === "leader")?.site : undefined);
        if (led) break;
      }
    }
    out.push({ at: t, killed: leader, newLeader: led, ms: Date.now() - t, terms: after.map((s) => s.term) });
    log(`kill ${i}: ${leader} -> ${led} in ${Date.now() - t} ms`);
    if (i < n - 1) await sleep(gap);
  }
  return out;
}

/** The A1 replay with a lease per agent, run for one actor kind, with leader kills. */
/** Sixth pass: `--learners 1,2` places the A1 replay's learners (M0-WP9 B0-29 ran slots 1 and 2) on L0-12's groups. */
const l012Learners = (c: Ctx) => (c.opt.learners ? c.opt.learners.split(",").map(Number) : []);
async function feedLearners(group: string, learners: number[]) {
  for (let i = 0; i < 60 && learners.length; i++) {
    await call("/op", { group, op: { kind: "put", key: "warm", value: String(i) } }).catch(() => {});
    const ls = await call<{ isLearner?: boolean }[]>("/learnerstats", { group, learners }).catch(() => []);
    if (ls.length === learners.length && ls.every((l) => l.isLearner)) return i;
    await sleep(2_000);
  }
  return -1;
}
async function a1Leases(c: Ctx, kind: Kind, group: string, seconds: number, clients: number, extra: Partial<ReturnType<typeof a1>> = {}) {
  const learners = l012Learners(c);
  // Every poller sends `paths` (one agent's scope): a poll without `paths` batches for FEED_BATCH_ALL (10 s), so it
  // cannot see an event within 2 s by construction.
  const s = spec({ run: `${c.run}-${kind}`, group, seconds, clients }, { ...a1(500), learners, kind, leases: true, renewEvery: 300, leaseTtl: 600_000,
    mergePrs: 200, allPaths: true, races: kind === "grun" ? RACES : 0, ...extra });
  await start(s);
  log(`a1 ${kind}: started ${s.run}, ${seconds} s, start ${new Date(s.start).toISOString()}`);
  const killer = kills(group, 2, Math.max(60_000, (seconds * 1000) / 3), s.start - Date.now() + Math.max(60_000, (seconds * 1000) / 3));
  // Third pass: the group's leader and term once a second through the run, to count leader changes after each kill.
  const leaderLog: [number, string | null, number][] = [];
  const sampler = (async () => {
    while (Date.now() < s.start + seconds * 1000) {
      const st = await call<Status[]>("/status", { group }, 10_000).catch(() => [] as Status[]);
      const l = st.find((x) => x.role === "leader");
      leaderLog.push([Date.now() - s.start, l?.site ?? null, l?.term ?? Math.max(0, ...st.map((x) => x.term ?? 0))]);
      await sleep(1_000);
    }
  })();
  // Per-member feed state every 20 s (second pass: to trace a feed tail to a member).
  const timeline: unknown[] = [];
  const members = await watch(group, learners, s.start + seconds * 1000 + 5_000, async (t) => {
    const r = await call<{ voters: any[]; learners: any[] }>("/w9stats", { group, learners, reset: false }, 30_000).catch(() => null);
    if (r) timeline.push([t - s.start, ...[...r.voters, ...r.learners].map((v) => [v.site, v.role, v.term, v.feedServed, v.feedHeldNow, v.following, v.gap, v.applied, v.feedRefused, v.lagRefused, v.lagStarts])]);
  });
  const killed = await killer;
  await sampler;
  await sleep(60_000);
  const cs = await results(s.run, clients);
  const pushes = pushRows(cs);
  const feedLat = cs.flatMap((x) => x.feedLat ?? []);
  // Feed latency outside the kill windows (from 2 s before a kill to 30 s after it), and inside them.
  const windows = (killed as { at: number }[]).map((k) => [k.at - s.start - 2_000, k.at - s.start + 30_000]);
  const inKill = (t: number) => windows.some(([a, b]) => t >= a && t <= b);
  const latAt = cs.flatMap((x) => (x.feedLatAt ?? []) as number[]);
  const steady = feedLat.filter((_, i) => !inKill(latAt[i])), during = feedLat.filter((_, i) => inKill(latAt[i]));
  return { spec: { ...s, learners: undefined }, kills: killed,
    commitMs: commitDist(members.voters), pushes: { n: pushes.length, ok: pushes.filter((r) => r[4] === 0).length, refused: pushes.filter((r) => r[4] === 2).length,
      stale: pushes.filter((r) => r[4] === 1).length, error: pushes.filter((r) => r[4] === 3).length, clientMs: dist(pushes.map((r) => r[3])) },
    rules: rulesOf(cs), feed: feedOf(cs), feedLatMs: dist(feedLat), feedLatOver2s: feedLat.filter((v) => v > 2_000).length,
    feedLatSteady: dist(steady), feedLatSteadyOver2s: steady.filter((v) => v > 2_000).length, feedLatInKills: dist(during),
    pushesSteady: (() => { const r = pushes.filter((x) => !inKill(x[0])); return { n: r.length, ok: r.filter((x) => x[4] === 0).length }; })(),
    leaseLog: cs.flatMap((x) => x.leaseLog ?? []).length,
    // Racing pairs: each grant [t, pair, holder, granted, expires, tries]; the gap from the previous holder's expiry to
    // the next grant is negative when two holders held the scope at once.
    races: (() => {
      const g = cs.flatMap((x) => (x.raceLog ?? []) as number[][]).sort((p, q) => p[1] - q[1] || p[3] - q[3]);
      const gaps: number[] = [];
      for (let i = 1; i < g.length; i++) if (g[i][1] === g[i - 1][1]) gaps.push(g[i][3] - g[i - 1][4]);
      return { grants: g.length, handoffs: gaps.length, gapMs: dist(gaps), overlapping: gaps.filter((v) => v < 0).length, worstOverlapMs: Math.min(0, ...gaps) };
    })(),
    members: members.voters.map((v) => ({ site: v.site, role: v.role, term: v.term, leaderStarts: v.leaderStarts, xAlarms: v.xAlarmCount, overlaps: v.overlaps, revived: v.revived,
      extends: v.extends, stallMarks: v.stallMarks, metaLoads: v.metaLoads, feedOnLeader: v.feedOnLeader, feedHeldPeak: v.feedHeldPeak, lagStarts: v.lagStarts, lagStopped: v.lagStopped, lagRefused: v.lagRefused, lagMax: v.lagMax, lagAtAppend: v.lagAtAppend, lagAtPoll: v.lagAtPoll, lastResort: v.lastResort, commitFailHeld: v.commitFailHeld, appendFails: v.appendFails, appendSlow: v.appendSlow, learnerCalls: v.learnerCalls, feedRefused: v.feedRefused, feedRefusedBehind: v.feedRefusedBehind, feedServed: v.feedServed, events: (v.events ?? []).filter((e: string) => /STEP_DOWN|won term|ready|snapshot/.test(e)).slice(0, 40) })),
    learners: members.learners.map((v) => ({ slot: v.slot, site: v.site, isLearner: v.isLearner, feedServed: v.feedServed, feedHeldPeak: v.feedHeldPeak, feedFull: v.feedFull,
      feedRefused: v.feedRefused, feedRefusedBehind: v.feedRefusedBehind, lagStarts: v.lagStarts, lagStopped: v.lagStopped, lagRefused: v.lagRefused, lagMax: v.lagMax,
      lagAtAppend: v.lagAtAppend, lastResort: v.lastResort, gaps: v.gaps, snapInstalls: v.snapInstalls, error: v.error })),
    lastVoter: cs.reduce((a, x) => a + (x.lastVoter ?? 0), 0),
    errors: cs.reduce((a: Record<string, number>, x) => { for (const [k, n] of Object.entries((x.errors ?? {}) as Record<string, number>)) a[k] = (a[k] ?? 0) + n; return a; }, {}),
    timeline,
    // Feed latency in 30 s buckets of the event's leader time: [bucket s, events, over 2 s, over 3 s].
    feedLatBuckets: (() => { const b: Record<number, number[]> = {}; feedLat.forEach((v, i) => { const k = Math.floor(latAt[i] / 30_000) * 30; const x = (b[k] ??= [k, 0, 0, 0]); x[1]++; if (v > 2_000) x[2]++; if (v > 3_000) x[3]++; }); return Object.values(b); })(),
    feedLatSteadyOver3s: steady.filter((v) => v > 3_000).length,
    leaderLog,
    // Per kill: the distinct leaders sampled from the kill to the next kill (or the end), in order. One entry after the
    // killed leader is one change; more is a follow-on change.
    afterKills: (killed as { at: number; killed?: string }[]).map((k, i, ks) => {
      const from = k.at - s.start, to = i + 1 < ks.length ? ks[i + 1].at - s.start : seconds * 1000;
      const seq: string[] = [];
      for (const [t, l] of leaderLog) if (t >= from && t < to && l && seq[seq.length - 1] !== l) seq.push(l);
      const terms = leaderLog.filter(([t]) => t >= from && t < to).map((x) => x[2]);
      return { killed: k.killed, leaders: seq, termFrom: Math.min(...terms), termTo: Math.max(...terms),
        // Leaders after the killed one: the first is the failover, each further one a follow-on change.
        followOn: Math.max(0, seq.filter((l, j) => !(j === 0 && l === k.killed)).length - 1) };
    }),
  };
}

/** The stall: every member keeps running while every member-to-member call hangs, for `stallMs`. The edge still reaches
 *  members, so `suspect` reports and campaigns set markers, and a lease's holder sees writes time out. */
async function stall(c: Ctx, group: string, stallMs: number) {
  const before = await leases(group);
  const lead = before.find((d) => !d.error)!;
  const live = lead.leases.filter((l) => l.expires > lead.at);
  log(`stall: ${live.length} live leases before, leader applied ${lead.applied}`);
  const from = Date.now() + 5_000;
  await faults(group, { rules: [{ target: "call", mode: "hang", rate: 1, re: "^[abc]>[abcl]", hangMs: 5_000 }], from, until: from + stallMs });
  // Members cache the plan and refresh it on their next call, so a few writes before `from` load it everywhere; the
  // cached plan carries `from` and `until`, so it takes effect on time with no further refresh.
  // A group under A1 load always has an entry in flight. Writes every 200 ms from 3 s before the stall to 10 s into it
  // keep one in flight when the calls stop, as the replay would, and also load the plan into every member's cache.
  // Then one write every 10 s keeps the edge's `suspect` path busy: writes time out at the leader, the edge reports
  // `suspect`, and followers campaign.
  const writes: unknown[] = [];
  const w = (body: string) => c.x(group, { t: "Meta", kind: "comment", body }, "stall").then((r) => r.out?.ok ?? r.code, (e) => String(e).slice(0, 60));
  const dense: Promise<unknown>[] = [];
  while (Date.now() < from + 10_000) {
    const t = Date.now() - from;
    dense.push(w("around stall start").then((r) => writes.push([t, r])));
    await sleep(200);
  }
  while (Date.now() < from + stallMs) {
    await sleep(10_000);
    const t = Date.now() - from;
    const r = await w("during stall");
    if (writes.length < 600) writes.push([t, r]);
  }
  await Promise.race([Promise.all(dense), sleep(5_000)]);
  await faults(group, { rules: [] });
  // The first entry the group commits after the stall carries `ExtendLeases`; one write makes sure there is one.
  let first: unknown = null;
  for (let i = 0; i < 30 && !first; i++) {
    await sleep(2_000);
    const r = await c.x(group, { t: "Meta", kind: "comment", body: "after stall" }, "after").catch(() => null);
    if (r?.out?.ok) first = r;
  }
  await sleep(5_000);
  const after = await leases(group);
  const st = await c.status(group);
  const per = after.filter((d) => !d.error).map((d) => {
    const ext = d.extends.filter((e) => e[3] >= from);
    // Leases live at the recorded start of the stall (`since` of its extension), the ones the row's gate covers.
    const since = ext.length ? Math.min(...ext.map((e) => e[1])) : from;
    const moved = live.filter((l) => l.expires > since).map((l) => {
      const now = d.leases.find((x) => x.id === l.id);
      return { id: l.id, before: l.expires, after: now?.expires ?? null, delta: now ? now.expires - l.expires : null, liveAfter: !!now && now.expires > d.lastAt };
    });
    return { site: d.site, applied: d.applied, extends: ext, byTotal: ext.reduce((a, e) => a + e[2], 0), since, sinceAfterFrom: since - from,
      liveAtSince: moved.length, lapsedBeforeSince: live.length - moved.length,
      liveAfter: moved.filter((m) => m.liveAfter).length, expired: moved.filter((m) => !m.liveAfter).length,
      deltas: dist(moved.map((m) => m.delta ?? -1)), notMoved: moved.filter((m) => !m.delta).length };
  });
  return { stallMs, from, until: from + stallMs, liveBefore: live.length, writes, firstAfter: first, per, status: st.map((s) => `${s.site}:${s.role}:${s.term}`),
    recorded: per.map((p) => ({ site: p.site, by: p.byTotal })), stallActual: stallMs };
}

/** An idle 60 s lease, then 10 minutes of no writes on the group, then an overlapping Exclusive acquire. */
async function idle(c: Ctx, group: string, idleMs: number) {
  const sess = "idleA";
  const holder: Who = { cred: { t: "Installation", app: APP.grun, installation: INST.grun, task: sess } };
  const r1 = await c.x(group, { t: "Acquire", who: holder, paths: ["idle/zone/**"], refs: [], mode: "Exclusive", ttl: 60_000 }, "idle");
  log("idle lease", JSON.stringify(r1.out).slice(0, 200));
  const t0 = Date.now();
  await sleep(idleMs);
  const other: Who = { cred: { t: "Installation", app: APP.grun, installation: INST.grun, task: "idleB" } };
  const r2 = await c.x(group, { t: "Acquire", who: other, paths: ["idle/zone/sub/**"], refs: [], mode: "Exclusive", ttl: 600_000 }, "idle2");
  log("late acquire", JSON.stringify(r2.out).slice(0, 200));
  const d = await leases(group);
  return { idleMs: Date.now() - t0, first: r1.out, late: r2.out, lateOk: !!r2.out?.ok,
    extendsDuringIdle: d.map((m) => ({ site: m.site, ext: (m.extends ?? []).filter((e) => e[3] >= t0) })) };
}

/** 100 conflicting pushes and 100 conflicting REST ref writes into the victim task's Exclusive scope, per kind. */
async function conflicts(c: Ctx, group: string, kinds: Kind[], n: number) {
  const out: Record<string, unknown> = {};
  for (const kind of kinds) {
    const cmds: { cmd: XCmd; key: string }[] = [];
    for (let i = 0; i < n; i++) for (const surface of ["git", "rest"] as const) {
      const a = 400 + (i % 50);
      cmds.push({ key: `c/${kind}/${surface}/${i}`, cmd: { t: "Push", who: who(kind, a), ref: `${ref(kind, a)}-${surface}-${i}`, expect: null, value: `v${i}`,
        changed: [VICTIM.path, changedPath(a, i)], surface } });
    }
    const rs = await c.xmany(group, cmds, 20);
    const by = (s: string) => rs.filter((_, i) => cmds[i].key.includes(`/${s}/`));
    const tally = (xs: typeof rs) => xs.reduce((a: Record<string, number>, r) => { const k = !r.out ? `none:${r.code}` : r.out.ok ? "accepted" : r.out.rule; a[k] = (a[k] ?? 0) + 1; return a; }, {});
    out[kind] = { git: tally(by("git")), rest: tally(by("rest")), sample: { git: by("git")[0]?.out, rest: by("rest")[0]?.out } };
    log(`conflicts ${kind}: ${JSON.stringify(out[kind]).slice(0, 300)}`);
  }
  return out;
}

export const rows: Record<string, (c: Ctx) => Promise<void>> = {
  /** L0-12. One group per actor kind. Phases: setup, a1 (with kills), conflict, stall, idle, clocks. */
  "l0-12": async (c) => {
    const kinds = (c.opt.kind ? c.opt.kind.split(",") : KINDS) as Kind[];
    const seconds = Number(c.opt.seconds ?? 600), stallS = Number(c.opt.stall ?? 1800), clients = Number(c.opt.clients ?? 30);
    const phases = (c.opt.phase ?? "a1,stall,idle,conflict,clocks,final").split(",");
    for (const kind of kinds) {
      const group = `l012-${kind}-${c.run}`;
      const name = (p: string) => `l0-12-${c.run}-${kind}-${p}`;
      await c.phase(name("setup"), async () => {
        const st = await setup(group, l012Learners(c), seed(kind, 500, 200));
        const fed = await feedLearners(group, l012Learners(c));
        // The victim task's Exclusive lease, long-lived and renewed by the runner between phases.
        const v = await c.x(group, { t: "Acquire", who: victimWho(), paths: [VICTIM.scope], refs: [], mode: "Exclusive", ttl: 3_600_000 }, "victim");
        return { status: st.map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`), victim: v.out, learners: l012Learners(c), fedAfter: fed };
      });
      const renewVictim = async () => {
        const d = await leases(group);
        const vl = d.find((m) => !m.error)?.leases.find((l) => l.holder.session === VICTIM.session);
        if (vl) await c.x(group, { t: "Renew", who: victimWho(), id: vl.id, ttl: 3_600_000 }, "victim");
        else await c.x(group, { t: "Acquire", who: victimWho(), paths: [VICTIM.scope], refs: [], mode: "Exclusive", ttl: 3_600_000 }, "victim");
      };
      if (phases.includes("a1")) await c.phase(name("a1"), () => a1Leases(c, kind, group, seconds, clients));
      if (phases.includes("conflict")) await c.phase(name("conflict"), async () => { await renewVictim(); return conflicts(c, group, [kind], 100); });
      if (phases.includes("stall")) await c.phase(name("stall"), async () => { await renewVictim(); return stall(c, group, stallS * 1000); });
      if (phases.includes("idle")) await c.phase(name("idle"), () => idle(c, group, Number(c.opt.idle ?? 600) * 1000));
      if (phases.includes("clocks")) await c.phase(name("clocks"), () => c.clocks(group, 20));
      if (phases.includes("final")) await c.phase(name("final"), async () => {
        const st = await call<{ voters: any[] }>("/w9stats", { group, learners: [], reset: false });
        return st.voters.map((v) => ({ site: v.site, role: v.role, overlaps: v.overlaps, overlapIds: v.overlapIds?.slice(0, 50), revived: v.revived, xAlarms: v.xAlarms, extends: v.extends, stallMarks: v.stallMarks }));
      });
    }
  },
};

/** L0-13: lease limits, on one group with Graftyard-run tasks, a vendor App, users, and plan blocks. */
rows["l0-13"] = async (c) => {
  const group = `l013-${c.run}`;
  const name = (p: string) => `l0-13-${c.run}-${p}`;
  const N = Number(c.opt.n ?? 100);
  // Bed: 200 Graftyard-run tasks (t0..t199), the vendor's External tasks, writers u60000.. and non-writers u70000..,
  // and an extra App installation (app 501, inst 5001) that will be uninstalled while it holds leases and tasks.
  const writers = (i: number) => 60_000 + i, outsiders = (i: number) => 70_000 + i;
  const UNINST = { app: 501, inst: 5001 };
  await c.phase(name("setup"), async () => {
    const extra: XEv[] = [
      { t: "AgentApp", row: { app: UNINST.app, kind: "GraftyardRun", confine: null } },
      { t: "Install", installation: UNINST.inst, app: UNINST.app, active: true, push: true },
      { t: "TaskCreated", task: grunTask("long") }, { t: "TaskCreated", task: grunTask("prober") },
      { t: "Collab", user: 90_001, perm: "write" },
      ...Array.from({ length: 20 }, (_, i) => ({ t: "TaskCreated" as const, task: { ...grunTask(`u${i}`), app: UNINST.app, installation: UNINST.inst } })),
    ];
    const chunks = seed("grun", 200, 0, extra);
    const users: XEv[] = Array.from({ length: 600 }, (_, i) => ({ t: "Collab" as const, user: writers(i), perm: "write" as const }));
    for (let i = 0; i < users.length; i += 300) chunks.push(users.slice(i, i + 300));
    const st = await setup(group, [], chunks);
    return { status: st.map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`) };
  });
  const tally = (rs: { out: XOut | null; code: number }[]) => rs.reduce((a: Record<string, number>, r) => {
    const k = !r.out ? `none:${r.code}` : r.out.ok ? (r.out.lease ? "accepted" : `ok:${r.out.text ?? ""}`) : `${r.out.status}:${r.out.rule}`; a[k] = (a[k] ?? 0) + 1; return a; }, {});
  const acq = (w: Who, paths: string[], mode: "Exclusive" | "Shared" = "Exclusive", refs: string[] = []): XCmd => ({ t: "Acquire", who: w, paths, refs, mode, ttl: 600_000 });
  const tw = (s: string): Who => ({ cred: { t: "Installation", app: APP.grun, installation: INST.grun, task: s } });

  await c.phase(name("bad-acquires"), async () => {
    const cases: Record<string, XCmd[]> = {
      "glob **": Array.from({ length: N }, (_, i) => acq(tw(`t${i}`), ["**"])),
      "glob *": Array.from({ length: N }, (_, i) => acq(tw(`t${i}`), ["*"])),
      "glob */x": Array.from({ length: N }, (_, i) => acq(tw(`t${i}`), ["*/x"])),
      // A Graftyard-run token whose task claim names no task, and a writer's user token with no task.
      "no live task (App)": Array.from({ length: N }, (_, i) => acq(tw(`gone${i}`), [`nolive/${i}/**`])),
      "no live task (user)": Array.from({ length: N }, (_, i) => acq({ cred: { t: "User", user: writers(i) } }, [`nolive/u${i}/**`])),
      // An App-token acquire with no session: the vendor App sends no X-Graftyard-Agent-Session.
      "App token, no session": Array.from({ length: N }, (_, i) => acq({ cred: { t: "Installation", app: APP.vendor, installation: INST.vendor, task: null } }, [`nosess/${i}/**`], "Shared")),
    };
    // A second Exclusive lease for one task: first take one per task, then ask again with a disjoint scope.
    const firsts = await c.xmany(group, Array.from({ length: N }, (_, i) => ({ key: `first/${i}`, cmd: acq(tw(`t${100 + i}`), [`second/${i}/a/**`]) })));
    cases["second for one task"] = Array.from({ length: N }, (_, i) => acq(tw(`t${100 + i}`), [`second/${i}/b/**`]));
    const out: Record<string, unknown> = { firstsOk: tally(firsts) };
    for (const [k, cmds] of Object.entries(cases)) {
      const rs = await c.xmany(group, cmds.map((cmd, i) => ({ key: `${k}/${i}`, cmd })));
      out[k] = tally(rs);
      log(`L0-13 ${k}: ${JSON.stringify(out[k])}`);
    }
    return out;
  });

  await c.phase(name("plan-blocks"), async () => {
    // 600 plan blocks from accounts without push access (each its own draft PR body edit), then 600 from writers.
    const non = await c.xmany(group, Array.from({ length: 600 }, (_, i) => ({ key: `plan/non/${i}`, cmd: { t: "PlanBlock", who: { cred: { t: "User", user: outsiders(i) } }, pr: 1000 + i, paths: [`plan/n${i}/**`] } as XCmd })), 30);
    const wr = await c.xmany(group, Array.from({ length: 600 }, (_, i) => ({ key: `plan/w/${i}`, cmd: { t: "PlanBlock", who: { cred: { t: "User", user: writers(i) } }, pr: 2000 + i, paths: [`plan/w${i}/**`] } as XCmd })), 30);
    const d = await leases(group);
    const lead = d.find((m) => !m.error)!;
    const live = lead.leases.filter((l) => l.expires > lead.lastAt);
    const plan = live.filter((l) => l.intent.startsWith("plan block"));
    const fromNon = plan.filter((l) => l.holder.actor.t === "User" && (l.holder.actor as { user: number }).user >= 70_000).length;
    return { nonWriters: tally(non), writers: tally(wr), planLeasesLive: plan.length, fromNonWriters: fromNon, members: d.map((m) => ({ site: m.site, plan: m.leases.filter((l) => l.intent.startsWith("plan block") && l.expires > m.lastAt).length })) };
  });

  await c.phase(name("removed-holder"), async () => {
    // User 90001 has push access and a live task; it takes an Exclusive lease, then loses push access. At the next
    // `decide`, a Graftyard-run push into that scope must not be refused by the lease.
    const t: XEv[] = [{ t: "TaskCreated", task: { ...grunTask("removed"), requester: 90_001 } }];
    await c.x(group, { t: "Seed", events: t }, "seed");
    const holder: Who = { cred: { t: "User", user: 90_001 }, session: "removed" };
    const got = await c.x(group, acq(holder, ["removed/zone/**"]), "rm");
    const before = await c.xmany(group, Array.from({ length: N }, (_, i) => ({ key: `rmb/${i}`, cmd: { t: "Push", who: tw(`t${i}`), ref: `refs/heads/graftyard/t${i}/rm-before-${i}`, expect: null, value: "v", changed: ["removed/zone/a.go"], surface: "git" } as XCmd })));
    await c.x(group, { t: "Seed", events: [{ t: "Collab", user: 90_001, perm: null }] }, "seed");
    const after = await c.xmany(group, Array.from({ length: N }, (_, i) => ({ key: `rma/${i}`, cmd: { t: "Push", who: tw(`t${i}`), ref: `refs/heads/graftyard/t${i}/rm-after-${i}`, expect: null, value: "v", changed: ["removed/zone/a.go"], surface: "git" } as XCmd })));
    const listed = await call<{ out: unknown[] }>("/xread", { group, what: "leases", arg: ["removed/**"] });
    return { lease: got.out?.ok, beforeRemoval: tally(before), afterRemoval: tally(after), listedAfter: listed.out.length };
  });

  await c.phase(name("uninstall"), async () => {
    const w = (i: number): Who => ({ cred: { t: "Installation", app: UNINST.app, installation: UNINST.inst, task: `u${i}` } });
    const got = await c.xmany(group, Array.from({ length: 20 }, (_, i) => ({ key: `un/${i}`, cmd: acq(w(i), [`uninst/${i}/**`]) })));
    const shared = await c.xmany(group, Array.from({ length: 20 }, (_, i) => ({ key: `uns/${i}`, cmd: acq(w(i), [`uninst/${i}/docs/**`], "Shared") })));
    const before = await call<{ out: unknown[] }>("/xread", { group, what: "leases", arg: ["uninst/**"] });
    await c.x(group, { t: "Seed", events: [{ t: "Install", installation: UNINST.inst, app: UNINST.app, active: false, push: true }] }, "seed");
    const after = await call<{ out: unknown[] }>("/xread", { group, what: "leases", arg: ["uninst/**"] });
    const d = await leases(group);
    const tasks = d.find((m) => !m.error)!.tasks;
    const states = Array.from({ length: 20 }, (_, i) => tasks[`u${i}`]);
    // A Graftyard-run push of another task into the uninstalled App's scopes: no lease refuses it now.
    const push = await c.xmany(group, Array.from({ length: N }, (_, i) => ({ key: `unp/${i}`, cmd: { t: "Push", who: tw(`t${i}`), ref: `refs/heads/graftyard/t${i}/un-${i}`, expect: null, value: "v", changed: [`uninst/${i % 20}/a.go`], surface: "git" } as XCmd })));
    const mint = await Promise.all(Array.from({ length: 20 }, (_, i) => call<XOut>("/mint", { group, session: `u${i}` })));
    return { acquired: tally(got), shared: tally(shared), listedBefore: before.out.length, listedAfter: after.out.length,
      taskStates: states.reduce((a: Record<string, number>, s) => ((a[s] = (a[s] ?? 0) + 1), a), {}), pushesAfter: tally(push),
      mintAfter: mint.reduce((a: Record<string, number>, r) => { const k = r.ok ? "minted" : r.rule; a[k] = (a[k] ?? 0) + 1; return a; }, {}) };
  });

  await c.phase(name("renew-5h-start"), async () => {
    const holder = tw("long");
    const got = await c.x(group, acq(holder, ["longzone/**"]), "long");
    if (!got.out?.ok || !got.out.lease) throw new Error(`long lease: ${JSON.stringify(got)}`);
    const r = { group, id: got.out.lease.id, granted: got.out.lease.granted, holder, prober: tw("prober"), probePath: "longzone/sub/**",
      probeFile: "longzone/sub/a.go", proberRef: "refs/heads/graftyard/prober/p", everyMs: 300_000, forMs: 5 * 3_600_000 + 600_000 };
    await call("/renew", { name: c.run, spec: r });
    return { started: new Date().toISOString(), lease: got.out.lease };
  });
  log("L0-13: the 5 h renewal runs from an alarm; collect with --phase renew-5h-collect after 5 h 10 min");
  if (c.opt.phase === "renew-5h-collect") await c.phase(name("renew-5h"), async () => call("/renewsteps", { name: c.run }));
};

/** Per-member feed counters from a `watch` snapshot. */
const feedMembers = (m: { voters: any[]; learners: any[] }) => [...m.voters, ...m.learners].map((v) => ({
  site: v.site, slot: v.slot, role: v.role, isLearner: v.isLearner, events: (v.events ?? []).filter((e: string) => /STEP_DOWN|won term|vote lost|refused as suspected/.test(e)).slice(0, 40), feedServed: v.feedServed, feedOnLeader: v.feedOnLeader, feedFull: v.feedFull,
  feedRefused: v.feedRefused, feedHeldPeak: v.feedHeldPeak, overloaded: v.overloaded, appendErrs: v.appendErrs, xAlarms: v.xAlarmCount,
  reqs: v.reqs, reqsSeconds: v.reqsSeconds, term: v.term, perSec: v.perSec, born: v.born, ctors: v.ctors, stepHolds: v.stepHolds, excused: v.excused, excusedTrips: v.excusedTrips, commitFailHeld: v.commitFailHeld, appendFails: v.appendFails, appendSlow: v.appendSlow, learnerCalls: v.learnerCalls, lagStarts: v.lagStarts, lagStopped: v.lagStopped, lagRefused: v.lagRefused, lagMax: v.lagMax, lagAtAppend: v.lagAtAppend, lagAtPoll: v.lagAtPoll, lastResort: v.lastResort }));

/** LF-12: A1 on one `Tri` group with no learners plus 5,000 held polls with `paths`, then again after `place_learners`. */
rows["lf-12"] = async (c) => {
  const seconds = Number(c.opt.seconds ?? 300), clients = Number(c.opt.clients ?? 30);
  const name = (p: string) => `lf-12-${c.run}-${p}`;
  const runOne = async (label: string, base: string, learners: number[]) => {
    // Each phase gets its own group from run r4 on, so refs one phase pushed cannot refuse the next phase's pushes as
    // `Stale` (r2 and r3 shared one group across phases).
    // A numeric test: the string test `c.run < "r4"` also matched r10 and r11, whose later phases shared one group.
    const group = /^r[123]$/.test(c.run) ? base : `${base}-${label}`;
    if (group !== base) await setup(group, [], seed("grun", 500, 200));
    // Placement: the voters' learner slots, then wait until the leader has fed every learner (`isLearner`), so polls
    // routed to learners find them following. Runs r1 and r2 skipped this and their learners were never fed.
    await call("/learners", { group, slots: learners });
    for (let i = 0; i < 60 && learners.length; i++) {
      await call("/op", { group, op: { kind: "put", key: "warm", value: String(i) } }).catch(() => {});
      const ls = await call<{ isLearner?: boolean }[]>("/learnerstats", { group, learners }).catch(() => []);
      if (ls.length === learners.length && ls.every((l) => l.isLearner)) { log(`lf-12 ${label}: ${learners.length} learners fed`); break; }
      await sleep(2_000);
    }
    // Members' request counters restart with the measured phase, so `reqs` is this phase's traffic by method.
    await call("/w9stats", { group, learners, reset: true }).catch(() => {});
    const s = spec({ run: `${c.run}-${label}`, group, seconds, clients }, { ...a1(500), pollers: 5_000, allPaths: true, learners, kind: "grun", mergePrs: 200 });
    await start(s);
    log(`lf-12 ${label}: started ${s.run}, learners ${learners.join(",") || "none"}`);
    const m = await watch(group, learners, s.start + seconds * 1000 + 5_000);
    await sleep(45_000);
    const cs = await results(s.run, clients);
    const fm = feedMembers(m);
    const held = cs.flatMap((x) => x.feedHeld ?? []);
    return { group, spec: { ...s }, commitMs: commitDist(m.voters), feed: feedOf(cs), heldMs: dist(held), members: fm,
      peakHeld: Math.max(...fm.map((f) => f.feedHeldPeak ?? 0)), onLeader: fm.reduce((a, f) => a + (f.feedOnLeader ?? 0), 0),
      onFollowers: m.voters.filter((v) => v.role !== "leader").reduce((a, v) => a + (v.feedServed ?? 0), 0),
      overloaded: fm.reduce((a, f) => a + (f.overloaded?.append ?? 0) + (f.overloaded?.learner ?? 0), 0),
      pushes: (() => { const p = pushRows(cs); return { n: p.length, ok: p.filter((r) => r[4] === 0).length }; })(),
      // The edge's side: suspects sent after a missed receipt, `notLeader` redirects followed, commands given up, and
      // feed member calls (each answer counts its tries; each 429 counts the members it tried).
      client: { suspects: cs.reduce((a, x) => a + (x.suspectsSent ?? 0), 0), notLeaders: cs.reduce((a, x) => a + (x.notLeaders ?? 0), 0),
        gaveUp: cs.reduce((a, x) => a + (x.gaveUp ?? 0), 0), lastVoter: cs.reduce((a, x) => a + (x.lastVoter ?? 0), 0) },
      leaderChanges: Math.max(...fm.filter((f) => !f.isLearner && f.slot === undefined).map((f) => f.term ?? 0)),
      errors: cs.reduce((a: Record<string, number>, x) => { for (const [k, n] of Object.entries((x.errors ?? {}) as Record<string, number>)) a[k] = (a[k] ?? 0) + n; return a; }, {}) };
  };
  const g = `lf12-${c.run}`;
  const before = await c.phase(name("setup"), async () => ({ status: (await setup(g, [], seed("grun", 500, 200))).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`) }));
  void before;
  const noL = await c.phase(name("no-learners"), () => runOne("nolearn", g, []));
  // As written: learners until the reported peak fits in half of FEED_HELD_MAX each.
  const asWritten = await c.phase(name("place-written"), async () => {
    const r = noL as Awaited<ReturnType<typeof runOne>>;
    const peaks = r.members.filter((m) => !m.isLearner && m.role !== "leader").map((m) => m.feedHeldPeak ?? 0);
    return { peaks, learners: placeLearners({ peakHeld: peaks, learners: 0 }) };
  }) as { learners: number };
  await c.phase(name("placed-written"), () => runOne("placedw", g, Array.from({ length: asWritten.learners }, (_, i) => i + 1)));
  // `place_learners` from the members' reported peaks. A full member's peak is FEED_HELD_MAX however many polls
  // arrive, so the design's rule (peak / half of FEED_HELD_MAX) is computed both as written and with the refused polls'
  // demand added (outside the design).
  const placed = await c.phase(name("place"), async () => {
    const r = noL as Awaited<ReturnType<typeof runOne>>;
    const peaks = r.members.filter((m) => !m.isLearner && m.role !== "leader").map((m) => m.feedHeldPeak ?? 0);
    // Outside the design: demand is the pollers the edge saw, held or refused, not the members' peaks.
    const withDemand = placeLearners({ peakHeld: peaks, learners: 0, refusedHeld: Math.max(0, 5_000 - peaks.reduce((a, b) => a + b, 0)) });
    return { peaks, withDemand, slots: Array.from({ length: withDemand }, (_, i) => 101 + i) };
  });
  const p = placed as { slots: number[] };
  await c.phase(name("placed"), () => runOne("placed", g, p.slots));
};

/** LF-13: 1,000 polls held on a follower, its AppendEntries dropped for 5 s, then a gap and an InstallSnapshot. */
rows["lf-13"] = async (c) => {
  const name = (p: string) => `lf-13-${c.run}-${p}`;
  // "snap200-": the leader keeps 200 entries, so 5 s of entries at about 10 entries/s cannot be served from the log
  // only if the drop lasts longer than the log; the runner sends a burst during the drop to push the tail past it.
  const g = `snap10-lf13-${c.run}`;
  await c.phase(name("setup"), async () => ({ status: (await setup(g, [], seed("grun", 50, 0))).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`) }));
  await c.phase(name("run"), async () => {
    const st = await c.status(g);
    const follower = st.find((s) => s.role === "follower")!.site;
    const seconds = 90;
    // 1,000 pollers on one follower, each with `paths` matching every push (`pkg/**`), recording ring indices. With no
    // side commands, every ring row after the seed is a push under pkg/, so every row matches every poller.
    const s = spec({ run: `${c.run}-lf13`, group: g, seconds, clients: 10 }, { ...a1(50), pushEvery: 20, refReads: 0, apiReads: 0, agentReads: 0,
      mergesPerMin: 0, pollers: 1_000, allPaths: "pkg/**", learners: [], kind: "grun", pollSite: follower as "a" | "b" | "c", seenIdx: true, bigShare: 0, agentSide: 0 });
    await start(s);
    log(`lf-13: follower ${follower}, started ${s.run}`);
    await sleep(s.start - Date.now() + 30_000);
    const pre = await call<{ voters: any[] }>("/w9stats", { group: g, learners: [], reset: false });
    const heldBefore = pre.voters.find((v) => v.site === follower)?.feedHeldNow;
    // Drop every AppendEntries to the follower for 5 s. The group keeps 10 entries ("snap10-"), so the leader's next
    // AppendEntries starts past its compacted log.
    const from = Date.now() + 2_000;
    await faults(g, { rules: [{ target: "call", mode: "error", rate: 1, re: `^[abc]>${follower}:(append|installSnapshot)` }], from, until: from + 5_000 });
    await sleep(Math.max(0, from + 5_000 - Date.now()) + 15_000);
    const post = await call<{ voters: any[] }>("/w9stats", { group: g, learners: [], reset: false });
    await sleep(s.start + seconds * 1000 + 10_000 - Date.now());
    const cs = await results(s.run, 10);
    const fv = post.voters.find((v) => v.site === follower);
    // No event lost: every ring row after the first poll's cursor, as the leader's ring holds it, reached each poller
    // that polled across the gap. Compared against the union of indices any poller saw and the leader's ring.
    const leader = post.voters.find((v) => v.role === "leader")?.site;
    // No event lost: each poller's received ring indices, compared with the leader's ring rows (every entry carries a
    // push under pkg/, so every ring row matches `pkg/**`) between that poller's first and last received index.
    // The follower's own ring too: what it holds after the snapshot is what it can serve.
    const ring = await call<{ site: string; applied: number; rows: [number, number, number, string[]][] }>("/ringdump", { group: g, site: leader, from: 0 });
    const fring = await call<{ site: string; applied: number; rows: [number, number, number, string[]][] }>("/ringdump", { group: g, site: follower, from: 0 });
    const fset = new Set(fring.rows.map((r) => r[0]));
    const notOnFollower = ring.rows.filter((r) => !fset.has(r[0]) && r[0] <= fring.applied).map((r) => r[0]);
    const seenAll = cs.flatMap((x) => (x.seenBy ?? []) as { g: string; s: number[] }[]);
    let missing = 0, dup = 0, checked = 0;
    const missingSample: number[] = [];
    const gapIdx = fv?.gaps?.[0]?.[2] ?? Infinity;
    let span = 0;
    // Rows under pkg/ (agent pushes); the seed entries carry none.
    const mine = ring.rows.filter((r) => r[3].some((p) => p.startsWith("pkg/"))).map((r) => r[0]);
    for (const { s: sx } of seenAll) {
      if (!sx.length) continue;
      const set = new Set(sx);
      dup += sx.length - set.size;
      const lo = Math.min(...sx), hi = Math.max(...sx);
      if (lo < gapIdx && hi > gapIdx) span++;
      for (const i of mine) if (i >= lo && i <= hi) { checked++; if (!set.has(i)) { missing++; if (missingSample.length < 20) missingSample.push(i); } }
    }
    const seen = seenAll;
    const stopped = cs.flatMap((x) => (x.stoppedAt ?? []) as number[][]);
    const gapAt = fv?.gaps?.[0]?.[0] ?? null;
    return { follower, leader, heldBefore, gaps: fv?.gaps, snapInstalls: fv?.snapInstalls, feedStopped: fv?.feedStopped, feedRefusedBehind: fv?.feedRefusedBehind,
      stoppedAnswers: { n: stopped.length, heldMs: dist(stopped.map((x) => x[1])), receivedAfterGapMs: gapAt ? dist(stopped.map((x) => x[0] - gapAt)) : null },
      pollers: seen.length, spanningGap: span, ringRows: mine.length, checked, missing, missingSample, duplicates: dup, notOnFollower: notOnFollower.length,
      followerStops: fv?.stops,
      feed: feedOf(cs), snapshotsSent: post.voters.find((v) => v.role === "leader")?.snapshotsSent, fault: { from, until: from + 5_000 },
      errors: cs.reduce((a: Record<string, number>, x) => { for (const [k, n] of Object.entries((x.errors ?? {}) as Record<string, number>)) a[k] = (a[k] ?? 0) + n; return a; }, {}) };
  });
};

/** L0-16: pushes, merges, minting, and lease checks on the A1 replay, with the metadata tier unloaded ("core-" group). */
rows["l0-16"] = async (c) => {
  const seconds = Number(c.opt.seconds ?? 300), clients = Number(c.opt.clients ?? 30);
  const g = `core-l016-${c.run}`;
  const name = (p: string) => `l0-16-${c.run}-${p}`;
  await c.phase(name("setup"), async () => ({ status: (await setup(g, [], seed("grun", 500, 200))).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`) }));
  await c.phase(name("run"), async () => {
    const s = spec({ run: `${c.run}-l016`, group: g, seconds, clients }, { ...a1(500), pollers: 0, learners: [], kind: "grun", leases: true, renewEvery: 300,
      leaseTtl: 600_000, mergePrs: 200, sideMeta: true });
    await start(s);
    // Minting and lease checks from the runner during the load: 10/s mints and 10/s lease checks.
    const mint: Record<string, number> = {}, check: Record<string, number> = {};
    const end = s.start + seconds * 1000;
    await sleep(Math.max(0, s.start - Date.now()));
    const side = (async () => {
      while (Date.now() < end) {
        const a = Math.floor(Math.random() * 500);
        const [m, k] = await Promise.all([
          call<XOut>("/mint", { group: g, session: session("grun", a) }).then((r) => (r.ok ? "minted" : r.rule), (e) => `err:${String(e).slice(0, 40)}`),
          call<{ out: unknown[] }>("/xread", { group: g, what: "check", arg: [changedPath(a, 1)] }).then((r) => `ok:${r.out.length ? "leased" : "free"}`, (e) => `err:${String(e).slice(0, 40)}`)]);
        mint[m] = (mint[m] ?? 0) + 1; check[k] = (check[k] ?? 0) + 1;
        await sleep(100);
      }
    })();
    const m = await watch(g, [], end + 5_000);
    await side;
    await sleep(45_000);
    const cs = await results(s.run, clients);
    // The control: one metadata list read, which must load the tier and be counted.
    const before = m.voters.map((v) => ({ site: v.site, role: v.role, metaLoads: v.metaLoads, metaHits: v.metaHits, metaLoaded: v.metaLoaded, tables: v.tables }));
    const list = await call("/xread", { group: g, what: "meta", arg: [] });
    const after = await call<{ voters: any[] }>("/w9stats", { group: g, learners: [], reset: false });
    const p = pushRows(cs);
    return { rules: rulesOf(cs), leaderRules: m.voters.find((v) => v.role === "leader")?.xRules, mint, check, before,
      controlList: list, afterControl: after.voters.map((v) => ({ site: v.site, role: v.role, metaLoads: v.metaLoads, metaLoaded: v.metaLoaded, x_meta: v.tables?.x_meta })),
      pushes: { n: p.length, ok: p.filter((r) => r[4] === 0).length }, commitMs: commitDist(m.voters) };
  });
};

/** L0-01: gray enam (30% of calls to and from site A fail) during the A1 replay, with a base group and a fork group
 *  (agent pushes go to the fork, whose `decide` reads the base's task and leases as `deps`). */
rows["l0-01"] = async (c) => {
  const seconds = Number(c.opt.seconds ?? 300), clients = Number(c.opt.clients ?? 30);
  const base = `l001-${c.run}`, fork = `l001f-${c.run}-fork-of-${base}`;
  const name = (p: string) => `l0-01-${c.run}-${p}`;
  await c.phase(name("setup"), async () => ({
    base: (await setup(base, [], seed("grun", 500, 200))).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`),
    fork: (await setup(fork, [], [])).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`) }));
  await c.phase(name("run"), async () => {
    const s = spec({ run: `${c.run}-l001`, group: base, seconds, clients }, { ...a1(500), learners: [], kind: "grun", leases: true, renewEvery: 300,
      leaseTtl: 600_000, mergePrs: 200, pushAttempts: 3, forkGroup: fork, allPaths: true });
    await start(s);
    // The gray fault starts 90 s in and holds to the end: 30% of every member call to and from site A, and of the
    // edge's calls to A, fail, on both groups (labels "<caller>><callee>:<method>").
    const from = s.start + 90_000;
    const plan = { rules: [{ target: "call", mode: "error", rate: 0.3, re: "^(a>|[bcl0-9]*>a:|edge>a:)" }], from, until: s.start + seconds * 1000 };
    await faults(base, plan); await faults(fork, plan);
    // Leaders of both groups, sampled every second from 30 s before the fault, and member counters every 20 s.
    const samples: { at: number; base: string | null; fork: string | null }[] = [];
    const end = s.start + seconds * 1000;
    let mB: { voters: any[]; learners: any[] } = { voters: [], learners: [] }, mF = mB;
    const sampler = (async () => {
      await sleep(Math.max(0, from - 30_000 - Date.now()));
      while (Date.now() < end) {
        const [b, f] = await Promise.all([c.leaderOf(base).catch(() => null), c.leaderOf(fork).catch(() => null)]);
        samples.push({ at: Date.now(), base: b, fork: f });
        await sleep(1_000);
      }
    })();
    [mB, mF] = await Promise.all([watch(base, [], end + 5_000), watch(fork, [], end + 5_000)]);
    await sampler;
    await faults(base, { rules: [] }); await faults(fork, { rules: [] });
    await sleep(45_000);
    const cs = await results(s.run, clients);
    const movedAt = (k: "base" | "fork") => samples.find((x) => x.at >= from && x[k] && x[k] !== "a")?.at ?? null;
    const changes = (k: "base" | "fork") => { let n = 0, prev: string | null = "a"; for (const x of samples.filter((y) => y.at >= from)) if (x[k] && x[k] !== prev) { n++; prev = x[k]; } return n; };
    const p = pushRows(cs);
    const during = p.filter((r) => s.start + r[0] >= from);
    const within3 = (rs: number[][]) => rs.filter((r) => r[4] === 0 && r[9] <= 3).length;
    const events = (m: { voters: any[] }) => m.voters.map((v) => ({ site: v.site, role: v.role, term: v.term,
      events: (v.events ?? []).filter((e: string) => /STEP_DOWN|won term|pre-vote|vote lost/.test(e)).map((e: string) => `${((Number(e.split(" ")[0]) - from) / 1000).toFixed(1)} ${e.split(" ").slice(1).join(" ")}`).slice(0, 30),
      extends: v.extends, overlaps: v.overlaps, suspects: v.suspects }));
    return { from, plan, baseMovedMs: movedAt("base") ? movedAt("base")! - from : null, forkMovedMs: movedAt("fork") ? movedAt("fork")! - from : null,
      baseChanges: changes("base"), forkChanges: changes("fork"),
      pushes: { n: p.length, ok: p.filter((r) => r[4] === 0).length, okWithin3: within3(p), attempts: p.reduce((a: Record<number, number>, r) => ((a[r[9]] = (a[r[9]] ?? 0) + 1), a), {}) },
      pushesDuringFault: { n: during.length, ok: during.filter((r) => r[4] === 0).length, okWithin3: within3(during), codes: during.reduce((a: Record<number, number>, r) => ((a[r[4]] = (a[r[4]] ?? 0) + 1), a), {}) },
      rules: rulesOf(cs), commitBase: commitDist(mB.voters), commitFork: commitDist(mF.voters), base: events(mB), fork: events(mF), samples,
      errors: cs.reduce((a: Record<string, number>, x) => { for (const [k, n] of Object.entries((x.errors ?? {}) as Record<string, number>)) a[k] = (a[k] ?? 0) + n; return a; }, {}) };
  });
};

void AGENT_KINDS; void scope; void session; void grunTask; void pct;
export type { XEv, XOut };

/** Second pass, edit 6: `ExtendLeases.since` clamped to the leader's last entry `at` ("xclamp-" groups), under a 30-minute
 *  stall. Three groups at once: a full stall on an xclamp group (every member call hangs), an isolated follower on an
 *  xclamp group (calls to and from `b` hang while `a` and `c` keep committing), and the isolated follower on an unclamped
 *  control. Each holds 50 Exclusive leases that expire during the stall, and 50 other tasks acquire the same scopes once
 *  each lease has expired: during the stall where the group still commits, after it where it cannot. */
rows["clamp"] = async (c) => {
  const stallMs = Number(c.opt.stall ?? 1800) * 1000, N = 50;
  const name = (p: string) => `clamp-${c.run}-${p}`;
  const all = { full: `xclamp-full-${c.run}`, part: `xclamp-part-${c.run}`, ctl: `ctl-part-${c.run}` };
  const groups = Object.fromEntries(Object.entries(all).filter(([k]) => !c.opt.only || c.opt.only.split(",").includes(k)));
  const holder = (i: number) => who("grun", i), taker = (i: number) => who("grun", 100 + i);
  await c.phase(name("setup"), async () => Object.fromEntries(await Promise.all(Object.entries(groups).map(async ([k, g]) =>
    [k, (await setup(g, [], seed("grun", 200, 0))).map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`)]))));
  await c.phase(name("stall"), async () => Promise.all(Object.entries(groups).map(async ([k, g]) => {
    // Old holders: lease i expires (i + 1) * 30 s after it is granted, so all 50 expire inside the stall.
    const from = Date.now() + 15_000;
    const acq = await c.xmany(g, Array.from({ length: N }, (_, i) => ({ key: `old/${i}`, cmd: { t: "Acquire", who: holder(i), paths: [`cl/${i}/**`], refs: [],
      mode: "Exclusive", ttl: Math.max(60_000, (i + 1) * 30_000 + 15_000) } as XCmd })));
    const old = acq.map((r) => (r.out?.ok ? r.out.lease ?? null : null));
    const re = k === "full" ? "^[abc]>[abcl]" : "^([ac]>b|b>)";
    await faults(g, { rules: [{ target: "call", mode: "hang", rate: 1, re, hangMs: 5_000 }], from, until: from + stallMs });
    // Run c2 on: in the partial groups, the edge's commits to leader `a` also hang for the first 8 s of the stall, and
    // its `suspect` calls to `c` with them, so the edge reports `suspect` only to the isolated `b`. That gives `b` a
    // `stalled_since` from early in the stall that no extension covers, while `a` and `c` keep committing. When `b`
    // reconnects its marker reaches the leader, and without the clamp the extension's `since` precedes entries that
    // already judged the old leases expired (the revival path). Run c1 had no such marker, so its partial groups
    // recorded no extension.
    const trig = k !== "full" && c.run >= "c2";
    if (trig) {
      await faults(g, { rules: [{ target: "call", mode: "hang", rate: 1, re, hangMs: 5_000 }, { target: "call", mode: "hang", rate: 1, re: "^edge>(a:commit|c:suspect)", hangMs: 5_000 }], from, until: from + stallMs });
      void (async () => { await sleep(Math.max(0, from + 8_000 - Date.now())); await faults(g, { rules: [{ target: "call", mode: "hang", rate: 1, re, hangMs: 5_000 }], from, until: from + stallMs }); })();
    }
    const takes: unknown[] = [];
    const w = (body: string) => c.x(g, { t: "Meta", kind: "comment", body }, "w").then((r) => r.out?.ok ?? r.code, () => "err");
    // Writes keep entries flowing where the group still commits, and load the plan into every member before `from`.
    const writer = (async () => { while (Date.now() < from + stallMs) { await w("tick"); await sleep(Date.now() < from + 10_000 ? 300 : 5_000); } })();
    const take = async (i: number) => {
      const r = await c.x(g, { t: "Acquire", who: taker(i), paths: [`cl/${i}/sub/**`], refs: [], mode: "Exclusive", ttl: 600_000 }, `take/${i}`);
      return { i, at: Date.now(), out: r.out ? (r.out.ok ? "granted" : `${r.out.status}:${r.out.rule}`) : `none:${r.code}`, granted: r.out?.ok ? r.out.lease?.granted ?? null : null };
    };
    // Where the group commits through the stall, each taker acquires 5 s after its scope's lease expires.
    if (k !== "full") await Promise.all(old.map(async (l, i) => {
      if (!l) return;
      await sleep(Math.max(0, l.expires + 5_000 - Date.now()));
      if (Date.now() < from + stallMs) takes.push(await take(i));
    }));
    await writer;
    await faults(g, { rules: [] });
    // After the stall: every member reconnects, the isolated follower's marker reaches the leader, and the next entry
    // carries `ExtendLeases`. Then every taker that has no lease tries once.
    for (let i = 0; i < 10; i++) { await w("after"); await sleep(3_000); }
    const got = new Set((takes as { i: number; out: string }[]).filter((t) => t.out === "granted").map((t) => t.i));
    const after = await Promise.all(old.map((l, i) => (l && !got.has(i) ? take(i) : null)));
    for (let i = 0; i < 5; i++) { await w("settle"); await sleep(2_000); }
    const dump = await leases(g);
    const st = await call<{ voters: any[] }>("/w9stats", { group: g, learners: [], reset: false });
    // Overlaps read from each member's fold: two holders' Exclusive leases on one scope both live at its last `at`.
    const overl = dump.filter((d) => !d.error).map((d) => {
      let n = 0;
      for (let i = 0; i < N; i++) {
        const live = d.leases.filter((l) => l.mode === "Exclusive" && l.expires > d.lastAt && l.paths.some((p) => p.startsWith(`cl/${i}/`)));
        if (new Set(live.map((l) => l.holder.session)).size > 1) n++;
      }
      return { site: d.site, overlappingScopes: n, liveOld: d.leases.filter((l) => old.some((o) => o?.id === l.id) && l.expires > d.lastAt).length };
    });
    const tally = (xs: ({ out: string } | null)[]) => xs.reduce((a: Record<string, number>, t) => (t ? ((a[t.out] = (a[t.out] ?? 0) + 1), a) : a), {});
    return { group: g, kind: k, from, until: from + stallMs, oldGranted: old.filter(Boolean).length, takesDuring: tally(takes as { out: string }[]), takesAfter: tally(after),
      overlaps: overl, members: st.voters.map((v) => ({ site: v.site, role: v.role, term: v.term, revived: v.revived, overlaps: v.overlaps, extends: v.extends, stallMarks: v.stallMarks })) };
  })));
};
