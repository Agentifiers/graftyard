// M0-WP4 site-day runner, standing in for `graft-ops` (design/DESIGN.md, "Install operations", Site day, and journeys 5
// and 7; build/BUILD-PLAN.md, M0-WP4). It runs on graft-wp4-* only. Set RAFT_PREFIX=graft-wp4.
//
//   node run/site-day.ts setup [--groups 200] [--set main]     # create groups, leaders spread over a, b, c
//   node run/site-day.ts day --site b --release 2 [--reads 1] [--writes 1] [--break apply:new:|refuse] [--steps 1,10,50,100]
//       [--dwell 60] [--first-day 60] [--first-day-share 0] [--no-limit] [--gray c@10] [--home-back] [--shape 60] [--new-share 0]
//       [--set big] [--gate-from 1] --run X
//   node run/site-day.ts reset --site b                       # release 1 at 100% on that site
//   node run/site-day.ts guard --run X                        # B0-27
//
// A day: drain (drain key), upload N+1 with `wrangler versions upload`, deploy it at each step with `wrangler versions
// deploy N+1@p N@(100-p)`, dwell and judge the gates from the members' own counters, roll back (`N@100`) on a breach,
// then undrain under the 1% lead limit and dwell the "first day". Load runs throughout: 100 writes/s over every group.
// Durable Objects keep one version per deployment and reset when it changes, so a step resets about its share of the
// site's members (https://developers.cloudflare.com/workers/configuration/versions-and-deployments/gradual-deployments/).
// Prototype parameters, not the design's: dwell in seconds instead of STEP_DWELL's 2 h, STEP_MIN_ENTRIES 20, and
// automatic drain at a 10% missed-call share over 30 s from both observers instead of DRAIN_BURN's 10 x for 5 min.

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { LEASE, SITES, scriptName, type MemberStatus, type SiteKey } from "../src/raft/types.ts";
import { memberConfig, PREFIX, raft, RELEASE_1, wrangler, type ReleaseVars } from "./raft.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  groups: { type: "string", default: "200" }, set: { type: "string", default: "main" }, site: { type: "string" },
  release: { type: "string", default: "2" }, reads: { type: "string" }, writes: { type: "string" }, break: { type: "string" },
  steps: { type: "string", default: "1,10,50,100" }, dwell: { type: "string", default: "60" }, "first-day": { type: "string", default: "60" },
  "no-limit": { type: "boolean", default: false }, gray: { type: "string" }, "home-back": { type: "boolean", default: false },
  shape: { type: "string", default: "0" }, "new-share": { type: "string", default: "0" }, run: { type: "string" },
  "min-entries": { type: "string", default: "20" }, control: { type: "string" }, "drain-only": { type: "string" },
  lead: { type: "string" }, "gate-from": { type: "string", default: "1" }, "first-day-share": { type: "string", default: "0" },
} });
if (PREFIX === "graft-raft") throw new Error("site-day runs on graft-wp4-*: set RAFT_PREFIX=graft-wp4");
const ROOT = new URL("../", import.meta.url).pathname;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const run = opt.run ?? new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13);
mkdirSync(`${ROOT}results/raw`, { recursive: true });
const eventsPath = `${ROOT}results/raw/site-day-${run}.events.jsonl`;
// A finished or started run is never rerun under the same name (run it as a service with `...; sleep infinity`, so the
// supervisor does not restart it).
if ((positionals[0] === "day" || positionals[0] === "guard") && existsSync(eventsPath)) throw new Error(`run ${run} already started (${eventsPath})`);
const events: { at: number; what: string; [k: string]: unknown }[] = [];
function log(what: string, extra: Record<string, unknown> = {}): void {
  const e = { at: Date.now(), what, ...extra };
  events.push(e);
  appendFileSync(eventsPath, JSON.stringify(e) + "\n");
  console.log(new Date(e.at).toISOString().slice(11, 19), what, Object.keys(extra).length ? JSON.stringify(extra).slice(0, 300) : "");
}
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(s.length * p))] : null; };

// ---- groups ----

type GroupSet = { groups: string[]; home: Record<string, SiteKey> };
const setPath = (set: string) => `${ROOT}.gen/wp4-groups-${set}.json`;
const loadSet = (set = opt.set!): GroupSet => JSON.parse(readFileSync(setPath(set), "utf8"));

type Snap = { at: number; st: Record<string, (MemberStatus | { site: SiteKey; error: string })[]> };
async function snapshot(groups: string[], sites?: SiteKey[]): Promise<Snap> {
  const at = Date.now(), st: Snap["st"] = {};
  // At most 8 edge calls at once: each runs 60 member calls, and one invocation runs only 6 at a time.
  const chunks = Array.from({ length: Math.ceil(groups.length / 20) }, (_, i) => groups.slice(i * 20, i * 20 + 20));
  let next = 0;
  await Promise.all(Array.from({ length: 8 }, async () => {
    while (next < chunks.length) {
      const c = chunks[next++];
      for (let attempt = 0; attempt < 2; attempt++) {
        const r = await raft<Snap["st"]>("/statusMany", { groups: c, sites }, 30_000).catch(() => null);
        if (r) { Object.assign(st, r); break; }
        if (attempt) Object.assign(st, Object.fromEntries(c.map((g) => [g, []])));
      }
    }
  }));
  return { at, st };
}
const ok = (m: MemberStatus | { error: string }): m is MemberStatus => "role" in m;
/** The member that leads: role leader, or a reset leader that holds `led_term` for its current term and resumes it on
 *  its next request (design/DESIGN.md, `MemberState.led_term`); idle objects are evicted and rebuilt as followers. */
function leaderOf(ms: Snap["st"][string]): SiteKey | null {
  const ls = ms.filter(ok).filter((m) => m.role === "leader" || (m.ledTerm !== null && m.ledTerm === m.term)).sort((x, y) => y.term - x.term);
  return ls[0]?.site ?? null;
}

/** A write first: an idle group's members may have been evicted and rebuilt, and a member resumes leading only on a
 *  request (groups are request-driven), so `/transfer` would find no leader. */
async function transferTo(g: string, to: SiteKey): Promise<void> {
  await raft("/op", { group: g, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {});
  await raft("/transfer", { group: g, to }, 30_000).catch(() => {});
}

async function setup(): Promise<void> {
  const n = Number(opt.groups), tag = `${opt.set}-${Date.now() % 1e6}`;
  const groups = Array.from({ length: n }, (_, i) => `wp4-${tag}-${i}`);
  const home: Record<string, SiteKey> = Object.fromEntries(groups.map((g, i) => [g, SITES[i % 3]]));
  // The first write creates each group and elects A. Then each group not on its home moves there by transfer. A group
  // that shows no leader in a snapshot is only idle-looking: status reads one member at a time, and a member that has
  // not been called since a reset reports itself a follower, so the transfer finds the leader itself.
  for (let i = 0; i < n; i += 25) {
    await Promise.all(groups.slice(i, i + 25).map((g) => raft("/op", { group: g, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {})));
  }
  const todo = groups.filter((g) => home[g] !== "a");
  for (let i = 0; i < todo.length; i += 20) await Promise.all(todo.slice(i, i + 20).map((g) => transferTo(g, home[g])));
  for (let round = 0; round < 6; round++) {
    const snap = await snapshot(groups);
    const wrong = groups.filter((g) => leaderOf(snap.st[g]) !== home[g]);
    const errs = groups.filter((g) => snap.st[g].filter(ok).length < 3).length;
    if (errs) console.log(`  ${errs} groups with a member that did not answer status`);
    const dist: Record<string, number> = {};
    for (const g of wrong) { const k = `${leaderOf(snap.st[g]) ?? "none"}>${home[g]}`; dist[k] = (dist[k] ?? 0) + 1; }
    console.log(`round ${round} (${tag}): ${wrong.length} groups not led from home ${JSON.stringify(dist)}`);
    if (!wrong.length) break;
    for (let i = 0; i < wrong.length; i += 20) {
      await Promise.all(wrong.slice(i, i + 20).map(async (g) => {
        if (leaderOf(snap.st[g])) await transferTo(g, home[g]);
        else await raft("/op", { group: g, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {});
      }));
    }
    await sleep(2_000);
  }
  writeFileSync(setPath(opt.set!), JSON.stringify({ groups, home }));
  console.log(`wrote ${setPath(opt.set!)}`);
}

// ---- versions ----

function current(script: string): string {
  const d = JSON.parse(wrangler(["deployments", "status", "--name", script, "--json"]));
  const v = (d.versions as { version_id: string; percentage: number }[]).sort((x, y) => y.percentage - x.percentage)[0];
  return v.version_id;
}
function upload(site: SiteKey, vars: ReleaseVars): string {
  const out = wrangler(["versions", "upload", "-c", memberConfig(site, "cycle", vars, `r${vars.RELEASE}`), "--tag", `r${vars.RELEASE}`,
    "--message", `M0-WP4 release ${vars.RELEASE} ${vars.BREAK_KIND ?? ""}`.trim()]);
  const id = /Worker Version ID: ([0-9a-f-]{36})/.exec(out)?.[1];
  if (!id) throw new Error(`no version id in: ${out.slice(-300)}`);
  return id;
}
/** `immediate`: a Durable Object otherwise takes up a new version deferred, up to 5 minutes later by default
 *  (wrangler 4.x, `--durable-objects-code-update-mode`), so a step's dwell would judge members still on N. */
function deploy(script: string, split: [string, number][], message: string): void {
  wrangler(["versions", "deploy", ...split.filter(([, p]) => p > 0).map(([v, p]) => `${v}@${p}`), "--name", script, "-y", "--message", message,
    "--durable-objects-code-update-mode", "immediate"]);
}

// ---- Release group ----

async function mode(site: SiteKey, m: "drain" | "normal" | "limit" | "unlimit", by: "drain" | "customer"): Promise<{ ok?: boolean; value?: string | null }> {
  const r = await raft<{ out: { ok?: boolean; value?: string | null } }>("/op", { group: "release", op: { kind: "mode", key: "m", site, mode: m, by } }, 60_000);
  log(`mode ${by}-key ${m} ${site}`, r.out);
  return r.out;
}

// ---- load ----

type Op4 = [number, number, string, boolean, string?];
const slowTraces: unknown[] = [];
function startLoad(groups: string[], newShare: () => number): { stop: () => Promise<Op4[]> } {
  let running = true;
  const ops: Op4[] = [];
  const callers = Array.from({ length: 20 }, async (_, i) => {
    while (running) {
      const r = await raft<{ ops?: Op4[]; error?: string; slow?: unknown[] }>("/bench", { group: groups[0], groups, n: 100, concurrency: 5, ratePerWorker: 1,
        // Each caller stands in for an edge on one site's continent: its own SiteHealth observer and its own colo label
        // for `suspect`, so callers on two sites can make the two-colo report the design relies on.
        bytes: 1024, ops: true, observer: SITES[i % 3], colo: `edge-${SITES[i % 3]}`, newShare: newShare() }, 120_000).catch((e) => ({ error: String(e) }));
      if (r.ops) ops.push(...r.ops); else log("load call failed", { error: r.error?.slice(0, 200) });
      for (const x of (r.slow ?? []) as { group: string }[]) {
        if (slowTraces.length >= 40) break;
        // The members' own logs, at once: an idle group's objects are evicted within about a minute and lose them.
        const logs = await raft<{ site: string; log?: string[] }[]>("/stats", { group: x.group, reset: false }, 20_000).catch(() => []);
        slowTraces.push({ ...x, members: logs.map((m) => ({ site: m.site, log: (m.log ?? []).filter((l) => !/ ack /.test(l)).slice(-25) })) });
      }
    }
  });
  return { stop: async () => { running = false; await Promise.all(callers); return ops; } };
}

// ---- the day ----

interface Watch { snaps: Snap[]; leaderless: Map<string, number>; maxLeaderless: { group: string; ms: number } | null;
  /** Per group, each leader change seen: [snapshot time, leader]. */
  changes: Map<string, [number, SiteKey | null][]> }
function watcher(groups: string[]): { w: Watch; stop: () => Promise<void> } {
  const w: Watch = { snaps: [], leaderless: new Map(), maxLeaderless: null, changes: new Map() };
  const lastLed = new Map<string, number>();
  let running = true;
  const loop = (async () => {
    while (running) {
      const t = Date.now();
      const s = await snapshot(groups);
      w.snaps.push(s); if (w.snaps.length > 4) w.snaps.shift();
      for (const g of groups) {
        // A status call can fail while a member resets, so a group counts as leaderless only when all three answered.
        if (!leaderOf(s.st[g]) && s.st[g].filter(ok).length < 3) continue;
        const ch = w.changes.get(g) ?? [], l = leaderOf(s.st[g]);
        if (!ch.length || ch.at(-1)![1] !== l) { ch.push([s.at, l]); w.changes.set(g, ch); }
        if (leaderOf(s.st[g])) {
          const since = lastLed.get(g);
          // A gap runs from the last snapshot that showed a leader to this one.
          if (since !== undefined && w.leaderless.has(g)) {
            const ms = s.at - since;
            if (!w.maxLeaderless || ms > w.maxLeaderless.ms) w.maxLeaderless = { group: g, ms };
          }
          w.leaderless.delete(g);
          lastLed.set(g, s.at);
        } else w.leaderless.set(g, s.at);
      }
      await sleep(Math.max(0, 2_000 - (Date.now() - t)));
    }
  })();
  return { w, stop: async () => { running = false; await loop; } };
}
/** The watcher's latest snapshot taken at or after `after`, waiting for one. */
async function snapAfter(w: Watch, after: number): Promise<Snap> {
  while (!w.snaps.length || w.snaps.at(-1)!.at < after) await sleep(500);
  return w.snaps.at(-1)!;
}

interface Gate { nMinus: number; nPlus: number; applied: number; applyErrors: number; refused: number; cannotFollow: string[]; stepDowns: number; alarms: string[] }
/** `cannot_follow` marks that leaders already held when the day started; the gate counts only new ones. */
let cfBefore = new Set<string>();
const cfMarks = (s: Snap) => new Set(Object.entries(s.st).flatMap(([grp, ms]) => ms.filter(ok).filter((m) => m.role === "leader")
  .flatMap((m) => Object.keys(m.cannotFollow ?? {}).map((k) => `${grp}:${k}`))));
function gate(s: Snap, site: SiteKey, release: number, base: Map<string, number>): Gate {
  const g: Gate = { nMinus: 0, nPlus: 0, applied: 0, applyErrors: 0, refused: 0, cannotFollow: [], stepDowns: 0, alarms: [] };
  for (const [grp, ms] of Object.entries(s.st)) {
    for (const m of ms.filter(ok)) {
      if (m.site === site && m.release !== release) g.nMinus++;
      if (m.site === site && m.release === release) {
        g.nPlus++;
        const b = base.get(grp) ?? m.applied;
        if (!base.has(grp)) base.set(grp, m.applied);
        g.applied += m.applied - b;
        g.applyErrors += m.applyErrors ?? 0; g.refused += m.refusedByMe ?? 0; g.stepDowns += m.stepDowns ?? 0;
      }
      // Any site marked unable to follow halts the day: the rolling site (B0-09) or an older one (B0-10).
      const fresh = Object.keys(m.cannotFollow ?? {}).filter((k) => !cfBefore.has(`${grp}:${k}`));
      if (m.role === "leader" && fresh.length) {
        g.cannotFollow.push(`${grp}:${fresh.join("")}`);
        g.alarms.push(...(m.alarms ?? []).filter((a) => a.includes("cannot follow")).slice(-1));
      }
    }
  }
  return g;
}
const breach = (g: Gate) => g.applyErrors ? `apply exceptions ${g.applyErrors} on N+1` : g.refused ? `verify refusals ${g.refused} by N+1`
  : g.cannotFollow.length ? `${g.cannotFollow.length} groups mark the site unable to follow` : g.stepDowns ? `STEP_DOWN trips ${g.stepDowns} on N+1` : null;

type Health = Record<string, { calls: number; missed: number } | { error: string }>;
function burning(h: Health, t: SiteKey): boolean {
  return SITES.filter((o) => o !== t).every((o) => {
    const x = h[`${o}>${t}`];
    return x && "calls" in x && x.calls >= 30 && x.missed / x.calls >= 0.1;
  });
}

async function day(): Promise<void> {
  const S = opt.site as SiteKey;
  if (!SITES.includes(S)) throw new Error("--site a|b|c");
  const { groups, home } = loadSet();
  const release = Number(opt.release);
  const vars: ReleaseVars = { RELEASE: opt.release!, READS: opt.reads ?? opt.release!, WRITES: opt.writes ?? "1", ...(opt.break ? { BREAK_KIND: opt.break } : {}) };
  const steps = opt.steps!.split(",").map(Number), dwell = Number(opt.dwell) * 1000, minEntries = Number(opt["min-entries"]);
  const [grayT, grayAt] = opt.gray ? [opt.gray.split("@")[0] as SiteKey, Number(opt.gray.split("@")[1])] : [null, -1];
  const script = scriptName(PREFIX, S), N = current(script);
  // N+1 must be a higher release than N, so members report which side of the split they run. `reset --site` puts a
  // site back on release 1.
  const nRelease = Number(/env\.RELEASE \("(\d+)"\)/.exec(wrangler(["versions", "view", N, "--name", script]))?.[1] ?? 1);
  if (nRelease >= release) throw new Error(`site ${S} already runs release ${nRelease}; run \`node run/site-day.ts reset --site ${S}\` first`);
  log("day start", { site: S, vars, steps, dwellMs: dwell, N, groups: groups.length, set: opt.set });
  // --lead c:200: before the day, move 200 groups' leadership to site c (B0-07 needs groups led on the gray site).
  if (opt.lead) {
    const [site, n] = [opt.lead.split(":")[0] as SiteKey, Number(opt.lead.split(":")[1])];
    const pick = groups.filter((g) => home[g] === site).slice(0, n);
    for (let i = 0; i < pick.length; i += 20) await Promise.all(pick.slice(i, i + 20).map((g) => transferTo(g, site)));
    const s0 = await snapshot(pick);
    log(`moved leadership to ${site}`, { asked: pick.length, ledThere: pick.filter((g) => leaderOf(s0.st[g]) === site).length });
  }
  await raft("/faults", { plan: { rules: [] } });
  let newShare = Number(opt["new-share"]);
  const load = startLoad(groups, () => newShare);
  const watch = watcher(groups);
  const result: Record<string, unknown> = { row: "M0-WP4 site day", run, site: S, vars, steps, N };
  let autoDrain: Record<string, unknown> | null = null; // before --control returns, because finish() reads it
  if (opt["drain-only"]) result.row = "M0-WP4 drain only";
  await sleep(10_000);
  // --control N: the same load and watcher for N seconds with no drain and no deploy, as the baseline for a day.
  if (opt.control) { result.row = "M0-WP4 control"; await sleep(Number(opt.control) * 1000); return finish(); }
  const s0 = await snapAfter(watch.w, Date.now() - 5_000);
  const ledBefore = Object.entries(s0.st).filter(([, ms]) => leaderOf(ms) === S).map(([g]) => g);
  cfBefore = cfMarks(s0);
  if (cfBefore.size) log("cannot_follow already held at start (not gated)", { n: cfBefore.size, examples: [...cfBefore].slice(0, 5) });
  log("led on site before drain", { n: ledBefore.length });

  // Step 0: drain. Every leader on S hands off within one lease.
  const tDrain = Date.now();
  await mode(S, "drain", "drain");
  let drainMs: number | null = null;
  while (Date.now() - tDrain < 3 * LEASE) {
    const s = await snapAfter(watch.w, Date.now());
    if (Object.values(s.st).every((ms) => leaderOf(ms) !== S)) { drainMs = s.at - tDrain; break; }
  }
  // A group led on S that took no write since the drain still shows S as its leader: hand-off is request-driven. The
  // check that matters for B0-04 is that every write after the drain committed, which the load measures.
  const stillLed = Object.entries((await snapAfter(watch.w, 0)).st).filter(([, ms]) => leaderOf(ms) === S).map(([g]) => g);
  const why = await Promise.all(stillLed.slice(0, 3).map(async (g) => ({ g, log: (await raft<{ site: string; log?: string[] }[]>("/stats", { group: g, reset: false })
    .catch(() => [])).find((m) => m.site === S)?.log?.filter((l) => !/ ack /.test(l)).slice(-12) })));
  log("drained", { drainMs, stillLed: stillLed.length, why });
  result.drain = { ledBefore: ledBefore.length, drainMs, stillLed: stillLed.length };

  if (opt["drain-only"]) {
    await sleep(Number(opt["drain-only"]) * 1000);
    await mode(S, "normal", "drain");
    await sleep(20_000);
    return finish();
  }
  const up = upload(S, vars);
  log("uploaded", { version: up });
  const stepLog: Record<string, unknown>[] = [];
  let halted: string | null = null;
  let focus: ReturnType<typeof focusOn> | null = null;
  for (const p of steps) {
    deploy(script, [[up, p], [N, 100 - p]], `M0-WP4 ${run} step ${p}%`);
    const t0 = Date.now(), base = new Map<string, number>();
    log(`deployed ${p}%`);
    let g: Gate | null = null, why: string | null = null, grayFrom = 0;
    while (true) {
      const s = await snapAfter(watch.w, Math.max(t0, Date.now() - 1_000));
      g = gate(s, S, release, base);
      // --gate-from P: steps below P% only record their gate (B0-09 and B0-10 reach 10% before a refusal halts the day).
      why = p >= Number(opt["gate-from"]) ? breach(g) : null;
      if (why) break;
      if (p === grayAt && grayT && !grayFrom && Date.now() - t0 > 10_000) {
        grayFrom = Date.now() + 3_000;
        const rule = (match: string) => ({ target: "call", mode: "hang", rate: 0.5, match, hangMs: 3_000 });
        await raft("/faults", { plan: { rules: [rule(`>${grayT}:`), rule(`${grayT}>`)], from: grayFrom } });
        log("gray fault set", { site: grayT, from: grayFrom });
        focus = focusOn(Object.entries((await snapAfter(watch.w, Date.now() - 3_000)).st).filter(([, ms]) => leaderOf(ms) === grayT).map(([g]) => g), grayT);
      }
      if (grayT && grayFrom) {
        const h = await raft<Health>("/health", { windowMs: 30_000 }).catch(() => ({} as Health));
        if (burning(h, grayT)) { autoDrain = await preempt(S, grayT, grayFrom, h, watch.w, focus!); break; }
        if (Date.now() - grayFrom > 180_000) { why = "gray site never detected"; break; }
      }
      if (Date.now() - t0 >= dwell && (g.applied >= minEntries || p < Number(opt["gate-from"]))) break;
      if (Date.now() - t0 >= 2 * dwell) { why = `too few entries applied by N+1: ${g.applied} < ${minEntries} (STEP_MIN_ENTRIES)`; break; }
    }
    stepLog.push({ pct: p, ms: Date.now() - t0, gate: g, breach: why });
    log(`step ${p}% ${why ? `BREACH: ${why}` : autoDrain ? "preempted by automatic drain" : "passed"}`, { gate: g });
    if (autoDrain) { halted = "automatic drain"; break; }
    if (why) {
      halted = why;
      const tr = Date.now();
      deploy(script, [[N, 100]], `M0-WP4 ${run} rollback`);
      log("rolled back", { ms: Date.now() - tr });
      await mode(S, "normal", "drain");
      break;
    }
  }
  result.steps = stepLog; result.halted = halted; result.autoDrain = autoDrain;

  if (!halted) {
    // Undrain under the 1% lead limit, then the first day.
    if (!opt["no-limit"]) await mode(S, "limit", "drain");
    await mode(S, "normal", "drain");
    const t0 = Date.now(), base = new Map<string, number>();
    const ledAt0 = Object.fromEntries(Object.entries((await snapAfter(watch.w, t0)).st).map(([g, ms]) => [g, leaderOf(ms)]));
    newShare = Number(opt["first-day-share"]);
    if (newShare) log("first-day writes of the new kind", { share: newShare });
    let why: string | null = null;
    while (Date.now() - t0 < Number(opt["first-day"]) * 1000) {
      const s = await snapAfter(watch.w, Date.now() - 1_000);
      why = breach(gate(s, S, release, base));
      if (why) break;
    }
    newShare = 0;
    const s1 = await snapAfter(watch.w, Date.now());
    const changed = Object.keys(ledAt0).filter((g) => ledAt0[g] && leaderOf(s1.st[g]) && leaderOf(s1.st[g]) !== ledAt0[g]);
    result.firstDayLeaders = { groupsWithLeaderChange: changed.length, examples: changed.slice(0, 5),
      cannotFollow: gate(s1, S, release, new Map()).cannotFollow.length };
    log(`first day ${why ? `BREACH: ${why}` : "passed"}`, result.firstDayLeaders as Record<string, unknown>);
    result.firstDay = why ?? "passed";
    if (!opt["no-limit"]) await mode(S, "unlimit", "drain");
    if (why) result.halted = halted = `first day: ${why}`;
  }
  if (opt["home-back"]) {
    const s = await snapAfter(watch.w, Date.now());
    const away = groups.filter((g) => home[g] === S && leaderOf(s.st[g]) !== S);
    for (let i = 0; i < away.length; i += 20) await Promise.all(away.slice(i, i + 20).map((g) => transferTo(g, S)));
    await sleep(4_000);
    log("home groups transferred back", { tried: away.length, ledNow: Object.values((await snapAfter(watch.w, Date.now())).st).filter((ms) => leaderOf(ms) === S).length });
  }
  if (Number(opt.shape) > 0) result.shape = await shapePhase(S, groups, watch.w, (v) => { newShare = v; });
  return finish();

  async function finish(): Promise<void> {
  // Per second and per (observer, target), the member and edge calls that failed or missed CALL_DEADLINE, read while
  // load still runs: `SiteHealth` keeps its counts in memory, and an idle object is evicted with them.
  const t0 = events[0]?.at ?? Date.now();
  result.healthSeries = await raft("/health", { windowMs: Date.now() - t0, series: true }).catch((e) => String(e));
  const ops = await load.stop();
  await watch.stop();
  await raft("/faults", { plan: { rules: [] } });
  if (autoDrain) {
    // After the run, not part of the day: undrain the gray site (standing in for UNDRAIN_AFTER), put S back on N, and
    // lift its lead limit, so the next run starts from release N everywhere.
    await mode(autoDrain.target as SiteKey, "normal", "drain");
    deploy(script, [[N, 100]], `M0-WP4 ${run} cleanup`);
    await mode(S, "unlimit", "drain");
    log("cleanup: gray site undrained, rolling site back on N");
  }
  const okLat = ops.filter((o) => o[3]).map((o) => o[1] - o[0]);
  const final = await snapshot(groups);
  // A write slower than one LEASE is the clients' view of a group without a leader for more than one lease.
  result.load = { ops: ops.length, failed: ops.filter((o) => !o[3]).length, p50: pct(okLat, 0.5), p99: pct(okLat, 0.99), max: pct(okLat, 1),
    overLease: okLat.filter((l) => l > LEASE).length,
    slowest: ops.filter((o) => o[1] - o[0] > LEASE || !o[3]).slice(0, 20).map((o) => ({ at: new Date(o[0]).toISOString().slice(11, 19), ms: o[1] - o[0], ok: o[3], group: o[4] })),
    seconds: ops.length ? (Math.max(...ops.map((o) => o[1])) - Math.min(...ops.map((o) => o[0]))) / 1000 : 0 };
  // B0-04: latency by how long the group had gone without an operation before this one, to separate cold groups
  // (members evicted after about 10 s idle) from the deploy. Every op goes to a raw file for later analysis.
  writeFileSync(`${ROOT}results/raw/site-day-${run}.ops.jsonl`, ops.map((o) => JSON.stringify(o)).join("\n") + "\n");
  const lastAt = new Map<string, number>(), byGap: Record<string, number[]> = {};
  for (const o of [...ops].sort((x, y) => x[0] - y[0])) {
    const prev = o[4] ? lastAt.get(o[4]) : undefined;
    const gap = prev === undefined ? "first" : o[0] - prev < 2_000 ? "<2s" : o[0] - prev < 10_000 ? "2-10s" : o[0] - prev < 20_000 ? "10-20s" : ">=20s";
    if (o[4]) lastAt.set(o[4], o[0]);
    if (o[3]) (byGap[gap] ??= []).push(o[1] - o[0]);
  }
  // And by the site that led the group in the final snapshot (control runs move no leader): the client is one colo, so
  // a group led far from it, or from its nearest follower, pays more round trips' worth.
  const byLeader: Record<string, number[]> = {};
  for (const o of ops) if (o[3] && o[4]) (byLeader[leaderOf(final.st[o[4]]) ?? "none"] ??= []).push(o[1] - o[0]);
  result.latencyByLeader = Object.fromEntries(Object.entries(byLeader).map(([k, l]) => [k, { n: l.length, p50: pct(l, 0.5), p99: pct(l, 0.99), overLease: l.filter((x) => x > LEASE).length }]));
  result.latencyByIdleGap = Object.fromEntries(Object.entries(byGap).map(([k, l]) => [k, { n: l.length, p50: pct(l, 0.5), p99: pct(l, 0.99), overLease: l.filter((x) => x > LEASE).length }]));
  // B0-07: each polled group's move time against how long it had been idle at the drain. The drain alarm covers a
  // leader that served a request or committed an entry within DRAIN_IDLE (60 s); an idle group's leader is evicted
  // and moves on its next request.
  const ad = autoDrain as { polled?: { perGroupMs?: Record<string, number | null> } } | null, td = events.find((e) => e.what === `mode drain-key drain ${(autoDrain as { target?: string } | null)?.target}`)?.at;
  if (ad?.polled?.perGroupMs && td) {
    const lastEnd = new Map<string, number>();
    for (const o of ops) if (o[4] && o[1] < td) lastEnd.set(o[4], Math.max(lastEnd.get(o[4]) ?? 0, o[1]));
    const rows = Object.entries(ad.polled.perGroupMs).map(([g, ms]) => ({ g, ms, idleMs: lastEnd.has(g) ? td - lastEnd.get(g)! : null }));
    const split = (f: (r: { idleMs: number | null }) => boolean) => { const r = rows.filter(f), m = r.map((x) => x.ms).filter((x): x is number => x !== null).sort((x, y) => x - y);
      return { groups: r.length, notMoved: r.length - m.length, p50: m[m.length >> 1] ?? null, max: m.at(-1) ?? null, over20s: m.filter((x) => x > 20_000).length + r.length - m.length }; };
    result.moveByIdle = { activeWithin50s: split((r) => r.idleMs !== null && r.idleMs < 50_000), idleOver50s: split((r) => r.idleMs === null || r.idleMs >= 50_000),
      late: rows.filter((r) => r.ms === null || r.ms > 20_000) };
  }
  result.maxLeaderless = watch.w.maxLeaderless;
  result.slowTraces = slowTraces;
  result.leaderlessAtEnd = groups.filter((g) => !leaderOf(final.st[g])).length;
  result.finalLeaders = Object.fromEntries(SITES.map((k) => [k, groups.filter((g) => leaderOf(final.st[g]) === k).length]));
  result.events = events;
  writeFileSync(`${ROOT}results/raw/site-day-${run}.json`, JSON.stringify(result, null, 1));
  const { events: _e, ...short } = result;
  console.log(JSON.stringify(short, null, 1).slice(0, 4000));
  }
}

/** Automatic drain of `t` during S's day (journey 5): stop the steps, undrain S under the 1% lead limit, drain `t`
 *  with the drain key, and time how long groups led on `t` take to move. */
/** Polls `groups` every second until each is led off `t`, recording when. The full snapshot takes about 20 s on 1,000
 *  groups, too coarse for B0-07's 20 s gate. It reads only the other two sites' members: status calls to the gray site
 *  hang half the time, which stretched one poll to about 3 s. A member there that leads, or holds `led_term` for its
 *  current term, is the new leader: a later term on `t` would have raised its term. */
function focusOn(groups: string[], t: SiteKey): { moved: Map<string, number>; stop: () => Promise<void>; groups: string[] } {
  const moved = new Map<string, number>();
  let running = true;
  const loop = (async () => {
    while (running && moved.size < groups.length) {
      const left = groups.filter((g) => !moved.has(g)), t0 = Date.now();
      const s = await snapshot(left, SITES.filter((k) => k !== t));
      for (const g of left) {
        // An evicted leader still holding `led_term` counts only if no answering member is on a later term.
        const ms = s.st[g].filter(ok), l = leaderOf(s.st[g]), lt = ms.find((m) => m.site === l)?.term ?? 0;
        if (l && l !== t && ms.every((m) => m.term <= lt)) moved.set(g, s.at);
      }
      await sleep(Math.max(0, 1_000 - (Date.now() - t0)));
    }
  })();
  return { moved, groups, stop: async () => { running = false; await loop; } };
}

async function preempt(S: SiteKey, t: SiteKey, grayFrom: number, h: Health, w: Watch, focus: ReturnType<typeof focusOn>): Promise<Record<string, unknown>> {
  log("automatic drain triggered", { target: t, health: h, sinceGrayMs: Date.now() - grayFrom });
  const ledAtGray = w.snaps.find((s) => s.at >= grayFrom - 3_000) ?? w.snaps[0];
  const onT = Object.entries(ledAtGray.st).filter(([, ms]) => leaderOf(ms) === t).map(([g]) => g);
  await mode(S, "limit", "drain");
  await mode(S, "normal", "drain");
  const td = Date.now();
  const r = await mode(t, "drain", "drain");
  const ledAtDrain = Object.entries((await snapAfter(w, td - 3_000)).st).filter(([, ms]) => leaderOf(ms) === t).map(([g]) => g);
  let movedMs: number | null = null;
  while (Date.now() - td < 60_000) {
    const s = await snapAfter(w, Date.now());
    if (Object.values(s.st).every((ms) => leaderOf(ms) !== t)) { movedMs = s.at - td; break; }
  }
  const s = await snapAfter(w, 0);
  // Each group led on t at the drain: the first snapshot after the drain that shows it led elsewhere.
  const moves = ledAtDrain.map((g) => { const c = (w.changes.get(g) ?? []).find(([at, l]) => at > td && l && l !== t); return c ? c[0] - td : null; });
  const done = moves.filter((x): x is number => x !== null).sort((a, b) => a - b);
  const perGroup = { ledAtDrain: ledAtDrain.length, moved: done.length, notMoved: moves.length - done.length,
    p50: done[Math.floor(done.length / 2)] ?? null, max: done.at(-1) ?? null, over20s: done.filter((x) => x > 20_000).length };
  const drained = await raft<{ modes?: { drained: SiteKey[] } }[]>("/status", { group: "release" }).catch(() => []);
  await sleep(5_000);
  await focus.stop();
  const fromGray = focus.groups.map((g) => focus.moved.get(g)).map((at) => (at === undefined ? null : at - grayFrom));
  const okG = fromGray.filter((x): x is number => x !== null).sort((a, b) => a - b);
  const beforeDrain = focus.groups.filter((g) => (focus.moved.get(g) ?? Infinity) < td).length;
  const afterDrain = focus.groups.map((g) => focus.moved.get(g)).filter((at): at is number => at !== undefined && at >= td).map((at) => at - td).sort((a, b) => a - b);
  const polled: Record<string, unknown> = { ledOnTargetAtGray: focus.groups.length, movedBeforeDrain: beforeDrain, movedAfterDrain: afterDrain.length,
    notMoved: fromGray.filter((x) => x === null).length, msFromGray: { p50: okG[okG.length >> 1] ?? null, max: okG.at(-1) ?? null },
    msFromDrain: { p50: afterDrain[afterDrain.length >> 1] ?? null, max: afterDrain.at(-1) ?? null, over20s: afterDrain.filter((x) => x > 20_000).length } };
  // Each polled group's move time after the drain, and the members' own logs for any that took over 20 s or never
  // moved, read now with retries: on the gray site a status or stats call hangs half the time.
  const lateMoves = focus.groups.filter((g) => (focus.moved.get(g) ?? Infinity) - td > 20_000);
  const late: Record<string, unknown>[] = [];
  for (const g of lateMoves.slice(0, 10)) {
    const logs: Record<string, string[]> = {};
    for (let i = 0; i < 4 && Object.keys(logs).length < 3; i++) {
      for (const m of await raft<{ site: string; log?: string[] }[]>("/stats", { group: g, reset: false }, 20_000).catch(() => [])) {
        if (m.log) logs[m.site] = m.log.filter((l) => !/ ack /.test(l)).slice(-40);
      }
    }
    late.push({ group: g, movedMs: focus.moved.has(g) ? focus.moved.get(g)! - td : null, logs });
  }
  polled.perGroupMs = Object.fromEntries(focus.groups.map((g) => [g, focus.moved.has(g) ? focus.moved.get(g)! - td : null]));
  const out = { target: t, detectMs: td - grayFrom, drainOk: r.ok, polled, late, ledOnTargetAtGray: onT.length, perGroup, movedMsAfterDrain: movedMs,
    movedMsAfterGray: movedMs === null ? null : td + movedMs - grayFrom, stillOnTarget: Object.values(s.st).filter((ms) => leaderOf(ms) === t).length,
    leaderSites: Object.fromEntries(SITES.map((k) => [k, Object.values(s.st).filter((ms) => leaderOf(ms) === k).length])),
    releaseView: drained.map((m) => m.modes) };
  log("automatic drain done", out);
  return out;
}

/** B0-08 and B0-10: load where 20% of writes use the new record shape, and what happened to every group that a site on
 *  the new release led when it started. */
async function shapePhase(S: SiteKey, groups: string[], w: Watch, setShare: (v: number) => void): Promise<Record<string, unknown>> {
  const before = await snapAfter(w, Date.now());
  const led = Object.fromEntries(groups.map((g) => [g, leaderOf(before.st[g])]));
  setShare(0.2);
  log("shape load on", { ledBySite: Object.fromEntries(SITES.map((k) => [k, groups.filter((g) => led[g] === k).length])) });
  await sleep(Number(opt.shape) * 1000);
  setShare(0);
  const s = await snapAfter(w, Date.now());
  const moved: Record<string, number> = {}, kept: Record<string, number> = {};
  let noLead = 0, cannotFollow: Record<string, number> = {};
  for (const g of groups) {
    const was = led[g], now = leaderOf(s.st[g]);
    if (!was) continue;
    (was === now ? kept : moved)[was] = ((was === now ? kept : moved)[was] ?? 0) + 1;
    for (const m of s.st[g].filter(ok)) {
      if (m.noLeadUntil != null) noLead++;
      if (m.role === "leader") for (const k of Object.keys(m.cannotFollow ?? {})) cannotFollow[k] = (cannotFollow[k] ?? 0) + 1;
    }
  }
  const alarms = Object.values(s.st).flatMap((ms) => ms.filter(ok).flatMap((m) => m.alarms ?? [])).slice(0, 5);
  const out = { moved, kept, membersWithNoLeadUntil: noLead, leadersWithCannotFollow: cannotFollow, alarms, groupsWithLeader: groups.filter((g) => leaderOf(s.st[g])).length,
    leadersNow: Object.fromEntries(SITES.map((k) => [k, groups.filter((g) => leaderOf(s.st[g]) === k).length])) };
  log("shape load done", out);
  void S;
  return out;
}

// ---- B0-27 ----

async function guard(): Promise<void> {
  const result: Record<string, unknown> = { row: "B0-27", run };
  for (const k of SITES) { await mode(k, "normal", "customer"); }
  const drainA = await mode("a", "drain", "drain");
  const drainB = await mode("b", "drain", "drain");
  const st = await raft<MemberStatus[]>("/status", { group: "release" });
  const page = st.flatMap((m) => m.alarms ?? []).filter((a) => a.includes("drain guard")).slice(-1);
  result.drainKey = { first: drainA, second: drainB, page, drainedAfter: st.find((m) => m.role === "leader")?.modes };
  await mode("a", "normal", "drain");

  // A group whose voters are all drained by customer-key entries elects from cold, and again after its leader is lost.
  const g = `b0-27-${run}`;
  for (const k of SITES) await mode(k, "drain", "customer");
  await sleep(6_000); // POINTER_TTL: every isolate sees all three drained
  let t = Date.now();
  const first = await raft("/op", { group: g, op: { kind: "put", key: "k", value: "0" } }).then(() => Date.now() - t, (e) => String(e));
  const s0 = await raft<MemberStatus[]>("/status", { group: g });
  const leader = s0.find((m) => m.role === "leader")?.site;
  result.cold = { firstCommitMs: first, leader };
  // Lose the leader: every call to and from its site fails.
  const faultAt = Date.now() + 3_000;
  await raft("/faults", { plan: { rules: [{ target: "call", mode: "error", rate: 1, match: `>${leader}:` }, { target: "call", mode: "error", rate: 1, match: `${leader}>` }], from: faultAt } });
  await sleep(3_000);
  let ms: number | null = null, i = 0;
  while (Date.now() - faultAt < 60_000) {
    t = Date.now();
    const r = await raft("/op", { group: g, op: { kind: "put", key: "k", value: String(++i) } }, 40_000).then(() => true, () => false);
    if (r) { ms = Date.now() - faultAt; break; }
  }
  await raft("/faults", { plan: { rules: [] } });
  await sleep(2_500);
  const s1 = await raft<(MemberStatus | { error: string })[]>("/status", { group: g });
  result.afterLoss = { firstCommitAfterFaultMs: ms, leader: s1.filter(ok).find((m) => m.role === "leader")?.site };
  for (const k of SITES) await mode(k, "normal", "customer");
  result.events = events;
  writeFileSync(`${ROOT}results/raw/site-day-${run}.json`, JSON.stringify(result, null, 1));
  const { events: _e, ...short } = result;
  console.log(JSON.stringify(short, null, 1));
}

/** Puts a site on one release at 100%: release 1 by default, or `--release R --reads F` to roll it forward. */
async function reset(): Promise<void> {
  const S = opt.site as SiteKey;
  const vars: ReleaseVars = process.argv.includes("--release") ? { RELEASE: opt.release!, READS: opt.reads ?? opt.release!, WRITES: opt.writes ?? "1" } : RELEASE_1;
  const id = upload(S, vars);
  deploy(scriptName(PREFIX, S), [[id, 100]], `M0-WP4 reset to release ${vars.RELEASE}`);
  console.log(`site ${S} on release ${vars.RELEASE} reading ${vars.READS} (${id})`);
}

switch (positionals[0]) {
  case "setup": await setup(); break;
  case "day": if (!existsSync(setPath(opt.set!))) throw new Error("run setup first"); await day(); break;
  case "guard": await guard(); break;
  case "reset": await reset(); break;
  default: throw new Error("usage: node run/site-day.ts setup|day|guard|reset");
}
