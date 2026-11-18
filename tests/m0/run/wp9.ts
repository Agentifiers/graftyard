// M0-WP9 runner: agent load on the base (build/BUILD-PLAN.md, M0-WP9, rows B0-29, B0-32, B0-33).
//
//   node run/wp9.ts deploy                       # graft-wp9-a/b/c (two-phase), graft-wp9-load, bucket graft-wp9-stage
//   node run/wp9.ts run b0-29 [--seconds 600] [--clients 30] [--learners 1,2] [--run X]
//   node run/wp9.ts run b0-32 [--seconds 180] [--clients 30]
//   node run/wp9.ts run b0-33 [--seconds 300] [--clients 60]
//   node run/wp9.ts collect --run X              # reads the clients and members again for a run already started
//   node run/wp9.ts delete                       # deletes the four scripts and empties and deletes the bucket
//
// Long runs go in a supervised service that does not restart on exit:
//   amp orb service start wp9-x --command "cd tests/m0 && node run/wp9.ts run b0-29 --run X > results/raw/wp9-b0-29-X.log 2>&1; sleep infinity"
//
// Every run uses a new group named after the row and run, so earlier runs never share a log. Load comes from `Load9`
// objects in graft-wp9-load, created with location hints and run from their alarms (src/wp9/load.ts), so the orb only
// starts and collects. Raw results go to results/raw/wp9-<row>-<run>.json.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { parseArgs } from "node:util";
import type { Spec } from "../src/wp9/load.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: false, options: {
  seconds: { type: "string" }, clients: { type: "string" }, learners: { type: "string" }, run: { type: "string" },
  group: { type: "string" }, wait: { type: "string" }, variant: { type: "string" }, want: { type: "string" } } });
const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const PREFIX = "graft-wp9";
const URL_ = `https://${PREFIX}-load.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const BUCKET = "graft-wp9-stage";
const BUCKET_C = "graft-wp9-stage-c"; // apac, for B0-31's apac leader
const COMPAT = "2026-10-01";
const SITES = ["a", "b", "c"] as const;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const now = () => new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13);

function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });
}
async function call<T>(path: string, body: unknown = {}, timeoutMs = 120_000): Promise<T> {
  const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
  const text = await res.text();
  if (!res.ok) throw new Error(`${path}: ${res.status} ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

function deploy(): void {
  mkdirSync(`${ROOT}wp9`, { recursive: true });
  const member = (k: string, phase: "own" | "cycle") => {
    const path = `wp9/${PREFIX}-${k}-${phase}.jsonc`;
    writeFileSync(`${ROOT}${path}`, JSON.stringify({
      name: `${PREFIX}-${k}`, main: "../src/wp9/member.ts", compatibility_date: COMPAT, workers_dev: false, preview_urls: false,
      vars: { SITE: k }, limits: { cpu_ms: 300_000 },
      durable_objects: { bindings: (phase === "own" ? [k] : SITES).map((s) => ({ name: `MEMBER_${s.toUpperCase()}`, class_name: "RaftMember",
        ...(s === k ? {} : { script_name: `${PREFIX}-${s}` }) })) },
      migrations: [{ tag: "v1", new_sqlite_classes: ["RaftMember"] }],
      observability: { enabled: true },
    }, null, 2));
    return path;
  };
  try { wrangler(["r2", "bucket", "create", BUCKET, "--location", "enam"]); } catch { /* exists */ }
  try { wrangler(["r2", "bucket", "create", BUCKET_C, "--location", "apac"]); } catch { /* exists */ }
  for (const phase of ["own", "cycle"] as const) for (const k of SITES) wrangler(["deploy", "-c", member(k, phase)]);
  const load = `wp9/${PREFIX}-load.jsonc`;
  writeFileSync(`${ROOT}${load}`, JSON.stringify({
    name: `${PREFIX}-load`, main: "../src/wp9/load.ts", compatibility_date: COMPAT, workers_dev: true, preview_urls: false,
    limits: { cpu_ms: 300_000 },
    durable_objects: { bindings: [...SITES.map((s) => ({ name: `MEMBER_${s.toUpperCase()}`, class_name: "RaftMember", script_name: `${PREFIX}-${s}` })),
      { name: "LOAD", class_name: "Load9" }] },
    r2_buckets: [{ binding: "STAGE", bucket_name: BUCKET }, { binding: "STAGE_C", bucket_name: BUCKET_C }],
    migrations: [{ tag: "v1", new_sqlite_classes: ["Load9"] }],
    observability: { enabled: true },
  }, null, 2));
  wrangler(["deploy", "-c", load]);
  wrangler(["secret", "put", "M0_TOKEN", "-c", load], TOKEN);
}

type Status = { site: string; role: string; term: number; colo?: string; leader?: string; error?: string };

/** Creates the group and moves its leadership to `home` (site A, enam, unless B0-31 asks for C), as bootstrap does. */
async function setup(group: string, learners: number[], home = "a"): Promise<Status[]> {
  await call("/op", { group, op: { kind: "put", key: "warm", value: "1" } });
  for (let i = 0; i < 15; i++) {
    const st = await call<Status[]>("/status", { group });
    const leader = st.find((s) => s.role === "leader")?.site;
    if (leader === home) break;
    if (leader) await call("/transfer", { group, to: home }).catch(() => {});
    else await call("/op", { group, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {});
    await sleep(1_000);
  }
  await call("/learners", { group, slots: learners });
  for (let i = 0; i < 5; i++) await call("/op", { group, op: { kind: "put", key: "warm", value: String(i) } });
  return call<Status[]>("/status", { group });
}

/** Journey 9 at `agents` agents (design/AGENT-LAYER.md, journey A1; `scripts/cost_agents.py --a1`): agent pushes every
 *  2 min, 21.0 agent side commands/s per 500 agents (25.2 agent class minus 4.17 pushes), 1.25 human and 1.0 bot
 *  commands/s, 10 merges/min, 33 CI ref reads/s, 200 API reads/s, 43 agent reads/s, and 500 feed pollers. */
function journey9(agents: number): Omit<Spec, "run" | "group" | "index" | "clients" | "start" | "seconds"> {
  const f = agents / 500;
  return { agents, pushEvery: 120, pushers: [], packMin: 10_000, packMax: 70_000, bigShare: 0.05, bigBytes: 1 << 20,
    agentSide: 21.0 * f, human: 1.25, bot: 1.0, refReads: 33, apiReads: 200, agentReads: 43, mergesPerMin: 10, pollers: 500, learners: [] };
}

const HINTS: DurableObjectLocationHint[] = ["enam", "enam", "wnam", "weur", "apac"];
/** B0-31's leader is on site C (apac): its pushes arrive from shards on apac, so apac clients push. */
const homeOf = (row: string) => (row.startsWith("b0-31") ? "c" : "a");
function specFor(row: string, run: string, group: string, seconds: number, clients: number, learners: number[]): Spec {
  const base = { run, group, index: 0, clients, start: Date.now() + 30_000, seconds };
  const pushHint = homeOf(row) === "c" ? "apac" : "enam";
  const pushers = Array.from({ length: clients }, (_, i) => i).filter((i) => HINTS[i % HINTS.length] === pushHint);
  const out = specRow(row, base, learners);
  return { ...out, pushers, ...(homeOf(row) === "c" ? { leaderSite: "c" as const } : {}) };
}
function specRow(row: string, base: Pick<Spec, "run" | "group" | "index" | "clients" | "start" | "seconds">, learners: number[]): Spec {
  if (row === "b0-29") return { ...base, ...journey9(500), learners };
  if (row === "b0-32") return { ...base, ...journey9(500), learners, burst: { at: Math.min(60, base.seconds / 3), pushes: 500, bytes: 256 << 10, merges: 5 } };
  // B0-31: journey 9 with every agent push at 1 MiB (side commands, reads, and polls as in B0-29).
  if (row === "b0-31") return { ...base, ...journey9(500), learners, pushFixed: 1 << 20 };
  // B0-31 third pass, the adopted outage rule: the same, plus Human pushes at journey 9's human push rate (0.3/s) and
  // merges, each with a pack uniform in [64 KiB, 1 MiB], so every one is over INLINE_MAX_HOT.
  if (row === "b0-31o") return { ...base, ...journey9(500), learners, pushFixed: 1 << 20, humanPushes: 0.3, bigMin: 64 << 10 };
  if (row === "b0-33") return { ...base, ...journey9(5_000), agentReads: 0, pollers: 0, learners, fastUser: 100 };
  throw new Error(`unknown row ${row}`);
}

const pct = (xs: number[], p: number) => { if (!xs.length) return null; const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const dist = (xs: number[]) => ({ n: xs.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99), max: pct(xs, 1) });
const CLS = ["Human", "MergeQueue", "Bot", "Agent"];
const KINDS = ["push", "merge", "side"];
const CODES = ["ok", "stale", "refused", "error", "false"];

async function collect(row: string, run: string): Promise<void> {
  const meta = JSON.parse(readFileSync(`${ROOT}results/raw/wp9-${row}-${run}.spec.json`, "utf8")) as { spec: Spec; hints: string[]; before: Status[] };
  const spec = meta.spec;
  const clients = await Promise.all(Array.from({ length: spec.clients }, (_, i) => call<any>("/result", { run, i }).catch((e) => ({ error: String(e), index: i }))));
  const done = clients.filter((c) => c.done);
  const snapPath = `${ROOT}results/raw/wp9-${row}-${run}.members.json`;
  const saved = existsSync(snapPath) ? JSON.parse(readFileSync(snapPath, "utf8")) as { snap: { voters: any[]; learners: any[] }; status: Status[] } : null;
  const members = saved?.snap ?? await call<{ voters: any[]; learners: any[] }>("/w9stats", { group: spec.group, learners: spec.learners, reset: false });
  const after = saved?.status ?? await call<Status[]>("/status", { group: spec.group });
  const lc = clients.map((c) => c.done ? c : c.partial ?? {}).filter((c) => c.rows);
  const rows: number[][] = lc.flatMap((c) => c.rows);
  const T = spec.seconds * 1000;
  const sel = (f: (r: number[]) => boolean) => rows.filter(f);
  const byKindCls: Record<string, unknown> = {};
  for (let k = 0; k < 3; k++) for (let c = 0; c < 4; c++) {
    const rs = sel((r) => r[1] === k && r[2] === c);
    if (!rs.length) continue;
    const codes: Record<string, number> = {};
    for (const r of rs) codes[CODES[r[4]]] = (codes[CODES[r[4]]] ?? 0) + 1;
    byKindCls[`${KINDS[k]}:${CLS[c]}`] = { n: rs.length, perS: +(rs.length / spec.seconds).toFixed(2), codes,
      clientMs: dist(rs.map((r) => r[3])), leaderCommitMs: dist(rs.filter((r) => r[5] >= 0 && r[4] !== 2).map((r) => r[5])),
      staged: rs.filter((r) => r[7]).length };
  }
  const voters = members.voters;
  const leader = voters.find((v) => v.role === "leader");
  const allLat: Record<string, number[]> = { Human: [], MergeQueue: [], Bot: [], Agent: [] };
  for (const v of voters) if (v.lat) for (const c of CLS) allLat[c].push(...v.lat[c]);
  const commitAll = CLS.flatMap((c) => allLat[c]);
  const waited: Record<string, Record<string, number>> = {};
  for (const v of voters) if (v.waited) for (const c of CLS) for (const [w, n] of Object.entries(v.waited[c] as Record<string, number>)) {
    (waited[c] ??= {})[w] = (waited[c][w] ?? 0) + n;
  }
  const reqRate = (v: any) => {
    const total = Object.values((v.reqs ?? {}) as Record<string, number>).reduce((a, b) => a + b, 0);
    return { total, perS: +(total / v.reqsSeconds).toFixed(1), seconds: v.reqsSeconds, byMethod: v.reqs,
      perSNoOutcome: +((total - (v.reqs?.outcome ?? 0)) / v.reqsSeconds).toFixed(1) };
  };
  const entries: number[][] = voters.flatMap((v) => v.entries ?? []);
  const sum = (k: string) => voters.reduce((a, v) => a + (v[k] ?? 0), 0) + members.learners.reduce((a, l) => a + (l[k] ?? 0), 0);
  const feed: Record<string, number> = {};
  for (const c of lc) for (const [k, n] of Object.entries(c.feed as Record<string, number>)) feed[k] = (feed[k] ?? 0) + n;
  const texts: Record<string, number> = {};
  for (const c of lc) for (const [k, n] of Object.entries(c.texts as Record<string, number>)) texts[k] = (texts[k] ?? 0) + n;
  const errors: Record<string, number> = {};
  for (const c of lc) for (const [k, n] of Object.entries(c.errors as Record<string, number>)) errors[k] = (errors[k] ?? 0) + n;
  const readsOf = (k: "ref" | "api" | "agent") => ({ ok: lc.reduce((a, c) => a + (c.reads[k]?.[0] ?? 0), 0), failed: lc.reduce((a, c) => a + (c.reads[k]?.[1] ?? 0), 0),
    ms: dist(lc.flatMap((c) => c.reads[`${k}Ms`] ?? [])) });
  const pushes = sel((r) => r[1] === 0);
  const merges = sel((r) => r[1] === 1);
  // Leader requests over the load window (30 s after the start to 10 s before the end), from the per-second samples of
  // whichever member led; [7] is the member's request total, [8] its outcome calls, [9] its feed calls.
  const win: Record<string, unknown> = {};
  // Only consecutive samples at most 3 s apart count, so a member's time out of leadership is left out.
  for (const v of voters) {
    const ss = ((v.seconds ?? []) as number[][]).filter((x) => x.length > 7 && x[0] >= spec.start + 30_000 && x[0] <= spec.start + T - 10_000);
    let secs = 0, req = 0, out = 0, fd = 0, peak = 0;
    for (let i = 1; i < ss.length; i++) {
      const dt = (ss[i][0] - ss[i - 1][0]) / 1000;
      if (dt > 3 || ss[i][7] < ss[i - 1][7]) continue;
      secs += dt; req += ss[i][7] - ss[i - 1][7]; out += ss[i][8] - ss[i - 1][8]; fd += ss[i][9] - ss[i - 1][9];
      peak = Math.max(peak, (ss[i][7] - ss[i - 1][7]) / dt);
    }
    if (secs) win[v.site] = { seconds: +secs.toFixed(0), reqPerS: +(req / secs).toFixed(1), withoutOutcome: +((req - out) / secs).toFixed(1),
      feedPerS: +(fd / secs).toFixed(2), peakPerS: Math.round(peak) };
  }
  const summary: Record<string, unknown> = {
    leaderWindow: win,
    row, run, group: spec.group, at: new Date().toISOString(), seconds: spec.seconds, clients: spec.clients, clientsDone: done.length,
    clientColos: lc.map((c) => c.colo), hints: meta.hints, learners: spec.learners,
    offeredPerS: +(rows.length / spec.seconds).toFixed(1),
    commitMs: { all: dist(commitAll), ...Object.fromEntries(CLS.map((c) => [c, dist(allLat[c])])) },
    waitedEntries: waited,
    byKindCls,
    stalePushes: pushes.filter((r) => r[4] === 1).length, pushesOk: pushes.filter((r) => r[4] === 0).length,
    pushesRefused: pushes.filter((r) => r[4] === 2).length, pushesError: pushes.filter((r) => r[4] === 3).length,
    merges: { n: merges.length, ok: merges.filter((r) => r[4] === 0).length, stale: merges.filter((r) => r[4] === 1).length,
      clientMs: dist(merges.map((r) => r[3])), leaderCommitMs: dist(merges.filter((r) => r[5] >= 0).map((r) => r[5])),
      waited: merges.map((r) => r[6]).reduce((a: Record<number, number>, w) => ((a[w] = (a[w] ?? 0) + 1), a), {}) },
    reads: { ref: readsOf("ref"), api: readsOf("api"), agent: readsOf("agent") },
    leader: leader ? { site: leader.site, term: leader.term, reqs: reqRate(leader), feedOnLeader: leader.feedOnLeader,
      capacityAtEnd: leader.capacity, roundEwma: leader.roundEwma, cmdBytesEwma: leader.cmdBytesEwma,
      agentQueuedPeak: leader.agentQueuedPeak, agentBytesPeak: leader.agentBytesPeak, demoted: leader.demoted, staged: leader.staged, budgetStaged: leader.budgetStaged, heldPeak: Math.max(0, ...voters.map((v) => v.heldPeak ?? 0)), holdExpired: voters.reduce((x, v) => x + (v.holdExpired ?? 0), 0),
      queueRefused: leader.queueRefused, refusals: leader.refusals, shapes: leader.shapes, stale: leader.stale } : null,
    confirmMs: Object.fromEntries(voters.filter((v) => v.confirmMs?.length).map((v) => [v.site, dist(v.confirmMs)])),
    memberEvents: voters.map((v) => ({ site: v.site, events: (v.events ?? []).map((e: string) => { const [t, ...m] = e.split(" "); return `${((Number(t) - spec.start) / 1000).toFixed(1)} ${m.join(" ")}`; }) })),
    members: voters.map((v) => ({ site: v.site, role: v.role, term: v.term, leaderStarts: v.leaderStarts, suspects: v.suspects, reqs: v.error ? v.error : reqRate(v),
      feedServed: v.feedServed, feedOnLeader: v.feedOnLeader, feedRefused: v.feedRefused, feedFull: v.feedFull, feedHeldPeak: v.feedHeldPeak })),
    learnerStats: members.learners.map((l) => ({ slot: l.slot, site: l.site, isLearner: l.isLearner, applied: l.applied, reqs: l.error ? l.error : reqRate(l),
      feedServed: l.feedServed, feedHeldPeak: l.feedHeldPeak, learnerAppends: l.learnerAppends })),
    leaderChanges: { before: meta.before.map((s) => `${s.site}:${s.role}:${s.term}`), after: after.map((s) => `${s.site}:${s.role}:${s.term}`),
      termChanged: Math.max(...after.map((s) => s.term ?? 0)) !== Math.max(...meta.before.map((s) => s.term ?? 0)),
      boots: { before: meta.before.map((s: any) => s.boots), after: after.map((s: any) => s.boots) },
      leaderStartsDuringRun: sum("leaderStarts") },
    suspects: { received: sum("suspects"), sentByClients: lc.reduce((a, c) => a + (c.suspectsSent ?? 0), 0), notLeaders: lc.reduce((a, c) => a + (c.notLeaders ?? 0), 0),
      gaveUp: lc.reduce((a, c) => a + (c.gaveUp ?? 0), 0), fallbacks: lc.reduce((a, c) => a + (c.fallbacks ?? 0), 0) },
    snapshotTakenAt: (members as any).takenAt ? Object.fromEntries(Object.entries((members as any).takenAt as Record<string, number>).map(([k, t]) => [k, +((t - spec.start) / 1000).toFixed(0)])) : null,
    feed: { polls: feed, pollsPerS: +(Object.entries(feed).filter(([k]) => k.startsWith("ok")).reduce((a, [, n]) => a + n, 0) / spec.seconds).toFixed(1),
      onLeader: sum("feedOnLeader"), at429s: lc.flatMap((c) => c.feed429At ?? []).map((t: number) => Math.round(t / 1000)).sort((a: number, b: number) => a - b).slice(0, 100), heldMs: dist(lc.flatMap((c) => c.feedHeld ?? [])), events: lc.reduce((a, c) => a + (c.feedEvents ?? 0), 0) },
    entries: { n: entries.length, perS: +(entries.length / spec.seconds).toFixed(2), cmds: dist(entries.map((e) => e[1])), bytes: dist(entries.map((e) => e[2])),
      inlinePackBytes: dist(entries.map((e) => e[3])), agentInlineBytes: dist(entries.filter((e) => e.length > 8).map((e) => e[8])) },
    refusalShapes: texts, badShapes: lc.flatMap((c) => c.badShapes ?? []),
    stageMs: dist(lc.flatMap((c) => c.stageMs ?? [])),
    clientFlightMax: lc.map((c) => Math.max(0, ...(c.flight ?? []))), shed: lc.reduce((a, c) => a + (c.shed ?? 0), 0),
    firstErr: lc.map((c) => ({ i: c.index, colo: c.colo, firstErr: c.firstErr })).filter((x) => x.firstErr && Object.keys(x.firstErr).length),
    lateSlots: lc.reduce((a, c) => a + (c.late?.length ?? 0), 0), alarmLateMs: lc.map((c) => c.alarmLateMs), errors,
  };
  if (spec.burst) {
    const b0 = spec.burst.at * 1000;
    const bp = sel((r) => r[1] === 0 && r[0] >= b0 - 50 && r[0] <= b0 + 1_100 && r[2] === 3);
    const bm = sel((r) => r[1] === 1 && r[0] >= b0 && r[0] <= b0 + 1_100);
    summary.burst = { pushes: bp.length, ok: bp.filter((r) => r[4] === 0).length, okWithin5s: bp.filter((r) => r[4] === 0 && r[3] <= 5_000).length,
      refused: bp.filter((r) => r[4] === 2).length, errors: bp.filter((r) => r[4] === 3).length, stale: bp.filter((r) => r[4] === 1).length,
      staged: bp.filter((r) => r[7]).length, clientMs: dist(bp.filter((r) => r[4] === 0).map((r) => r[3])),
      leaderCommitMs: dist(bp.filter((r) => r[4] === 0 && r[5] >= 0).map((r) => r[5])),
      merges: bm.map((r) => ({ t: r[0], code: CODES[r[4]], clientMs: r[3], leaderMs: r[5], waited: r[6] })) };
    // Leader samples around the burst: [at, capacity, tokens, agent queued, agent queued bytes, other queued, round EWMA].
    const lead = voters.find((v) => v.seconds?.length && v.role === "leader");
    summary.burstSeconds = (lead?.seconds ?? []).filter((s: number[]) => s[0] >= spec.start + b0 - 3_000 && s[0] <= spec.start + b0 + 40_000);
    // Eighth pass: stage latency of the burst's packs (stages that started in the burst second, 200 to 300 KiB), and
    // every merge's admission-to-commit wait in entries and ms.
    const bs = lc.flatMap((c) => (c.stages ?? []) as number[][]).filter((x) => x[0] >= spec.start + b0 - 50 && x[0] <= spec.start + b0 + 1_500 && x[2] >= 200_000 && x[2] <= 300_000);
    summary.burstStageMs = dist(bs.map((x) => x[1]));
    const allMerges = sel((r) => r[1] === 1 && r[4] === 0);
    summary.mergeWait = { leaderMs: dist(allMerges.filter((r) => r[5] >= 0).map((r) => r[5])), clientMs: dist(allMerges.map((r) => r[3])),
      entries: allMerges.map((r) => r[6]).reduce((a: Record<number, number>, w) => ((a[w] = (a[w] ?? 0) + 1), a), {}) };
  }
  if (spec.fastUser) {
    const fu = sel((r) => r[9] === 1);
    summary.fastUser = { n: fu.length, classedAgent: fu.filter((r) => r[2] === 3).length, afterFirstMinute: fu.filter((r) => r[0] > 60_000).length,
      agentAfterFirstMinute: fu.filter((r) => r[0] > 60_000 && r[2] === 3).length, codes: fu.reduce((a: Record<string, number>, r) => ((a[CODES[r[4]]] = (a[CODES[r[4]]] ?? 0) + 1), a), {}) };
  }
  // Admitted rates over the steady window (after 30 s, to 10 s before the end), by class, from the client rows.
  const w0 = 30_000, w1 = T - 10_000, ws = (w1 - w0) / 1000;
  summary.admittedPerS = Object.fromEntries(CLS.map((c, i) => [c, {
    offered: +(sel((r) => r[2] === i && r[0] >= w0 && r[0] < w1).length / ws).toFixed(1),
    committed: +(sel((r) => r[2] === i && r[0] >= w0 && r[0] < w1 && (r[4] === 0 || r[4] === 1)).length / ws).toFixed(1),
    refused: +(sel((r) => r[2] === i && r[0] >= w0 && r[0] < w1 && r[4] === 2).length / ws).toFixed(1) }]));
  // Third pass: refusal timing per class and kind, so "refused at once, never queued" is read from the client side.
  summary.refusedMs = Object.fromEntries(["push:Agent", "push:Human", "merge:MergeQueue", "side:Agent"].map((kc) => {
    const [k, c] = kc.split(":");
    const rs = sel((r) => r[1] === KINDS.indexOf(k) && r[2] === CLS.indexOf(c) && r[4] === 2);
    return [kc, { n: rs.length, clientMs: dist(rs.map((r) => r[3])) }];
  }));
  const raw = { summary, spec, members, clients: clients.map((c) => (c.done ? { ...c, rows: undefined } : c)), rows };
  writeFileSync(`${ROOT}results/raw/wp9-${row}-${run}.json`, JSON.stringify(raw));
  writeFileSync(`${ROOT}results/raw/wp9-${row}-${run}.summary.json`, JSON.stringify(summary, null, 2));
  console.log(JSON.stringify(summary, null, 2));
}

const [cmd, row] = positionals;
switch (cmd) {
  case "deploy": deploy(); break;
  case "run": {
    const run = (opt.run as string) ?? now();
    const seconds = Number(opt.seconds ?? (row === "b0-29" ? 600 : row === "b0-32" ? 180 : 300));
    const clients = Number(opt.clients ?? (row === "b0-33" ? 60 : 30));
    const learners = opt.learners ? String(opt.learners).split(",").map(Number) : [];
    // `--variant ih64-` prefixes the group name, which the members and clients read: INLINE_MAX_HOT of 64 KiB instead of
    // 256 KiB, outside the design (src/wp9/member.ts, `inlineMaxHot`).
    let group = (opt.group as string) ?? `${opt.variant ?? ""}${row}-${run}`;
    const home = homeOf(row);
    let before = await setup(group, learners, home);
    // Sixth pass: `--want IAD|ATL,AMS|ICN|NRT` keeps trying fresh groups (suffix g1, g2, ...) until the leader's colo
    // matches the first list and both followers' colos the second, up to 12 tries; a location hint does not pin a colo.
    if (opt.want) {
      const [lw, fw] = String(opt.want).split(",").map((x) => x.split("|"));
      // Seventh pass: a follower list starting with "+" needs only one follower in it (ATL, LHR, any).
      const anyOne = fw[0]?.startsWith("+"); if (anyOne) fw[0] = fw[0].slice(1);
      const fits = (b: Status[]) => b.some((x) => x.role === "leader" && lw.includes(x.colo ?? "")) &&
        (anyOne ? b.some((x) => x.role !== "leader" && fw.includes(x.colo ?? "")) : b.filter((x) => x.role !== "leader").every((x) => fw.includes(x.colo ?? "")));
      for (let g = 1; !fits(before) && g <= 12; g++) {
        console.log("placement", JSON.stringify(before.map((x) => `${x.site}:${x.role}:${x.colo}`)), "does not match; trying a fresh group");
        group = `${opt.variant ?? ""}${row}-${run}g${g}`;
        before = await setup(group, learners, home);
      }
    }
    console.log(JSON.stringify(before.map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`)));
    if (!before.some((s) => s.site === home && s.role === "leader")) throw new Error(`site ${home} does not lead`);
    await call("/w9stats", { group, learners, reset: true });
    const spec = specFor(row, run, group, seconds, clients, learners);
    const hints = HINTS;
    mkdirSync(`${ROOT}results/raw`, { recursive: true });
    writeFileSync(`${ROOT}results/raw/wp9-${row}-${run}.spec.json`, JSON.stringify({ spec, hints, before }, null, 2));
    // Eighth pass: a `/start` that times out has usually started all but one client (one `begin` hung, as in
    // b0-31-p8up); the started clients run on their own alarms, so the runner logs it and carries on to collect,
    // rather than throwing and having the batch script start a second run beside this one.
    const started = await call("/start", { spec, hints }, 150_000).catch((e) => ({ startError: String(e) }));
    console.log(JSON.stringify(started), `run ${run}, group ${group}, start ${new Date(spec.start).toISOString()}`);
    // Member counters are in memory and an idle member is evicted within seconds, so they are read 3 s after the load ends.
    // A member that resets loses its counters, so they are read every 20 s and the newest good reading of each member
    // is kept, with when it was taken.
    const keep: { voters: any[]; learners: any[]; takenAt: Record<string, number> } = { voters: [], learners: [], takenAt: {} };
    const take = async () => {
      const r = await call<{ voters: any[]; learners: any[] }>("/w9stats", { group, learners, reset: false }, 30_000).catch(() => null);
      if (!r) return;
      r.voters.forEach((v, i) => { if (!v.error && (v.reqsSeconds ?? 0) >= (keep.voters[i]?.reqsSeconds ?? 0) * 0.5) { keep.voters[i] = v; keep.takenAt[v.site] = Date.now(); } else keep.voters[i] ??= v; });
      r.learners.forEach((v, i) => { if (!v.error) keep.learners[i] = v; else keep.learners[i] ??= v; });
    };
    while (Date.now() < spec.start + seconds * 1000 + 3_000) { await sleep(20_000); await take(); }
    await take();
    const snap = keep;
    const logs = await call("/log", { group }, 60_000).catch((e) => ({ error: String(e) }));
    const status = await call("/status", { group });
    writeFileSync(`${ROOT}results/raw/wp9-${row}-${run}.members.json`, JSON.stringify({ at: Date.now(), snap, status, logs }));
    await sleep(Number(opt.wait ?? 75) * 1000);
    await collect(row, run);
    break;
  }
  case "collect": {
    const run = opt.run as string;
    if (!existsSync(`${ROOT}results/raw/wp9-${row}-${run}.spec.json`)) throw new Error("no spec for that run");
    await collect(row, run);
    break;
  }
  case "delete": {
    for (const bucket of ["a", "c"]) for (;;) { const r = await call<{ deleted: number; more: boolean }>("/purge", { prefix: "", bucket }); if (!r.deleted) break; }
    for (const s of ["load", "a", "b", "c"]) try { wrangler(["delete", `${PREFIX}-${s}`, "--force"]); } catch (e) { console.error(String(e)); }
    for (const b of [BUCKET, BUCKET_C]) wrangler(["r2", "bucket", "delete", b]);
    break;
  }
  default: throw new Error("usage: node run/wp9.ts deploy | run <row> | collect <row> --run X | delete");
}
