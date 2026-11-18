// B0-07 on the hosted code (graft-m3c, site build from branch m0-carried with `test-faults`): gray site C during site
// B's site day at 10%, with automatic drain. Stands in for `graft-ops` as Milestone 0's `tests/m0/run/site-day.ts`
// did, with the same prototype thresholds: C is drained once both other observers' `SiteHealth` show at least 10% of
// at least 30 calls to C missed over 30 s [guess, M0-WP4], instead of DRAIN_BURN's 10 x for 5 min.
//
//   node tests/m0carried/b07.ts setup --run R [--groups 200] [--on-c 80]   (groups whose home is C)
//   node tests/m0carried/b07.ts day --run R [--no-deploy]
//
// The day: load at 100 writes/s over every group from the orb through `graft-m3c-b07-edge` (each invocation counts
// its member calls into site 0's or site 1's health object, alternating, as edges on A and B would); drain B (site 1);
// upload a new version of graft-m3c-site-b and deploy it at 10% (`--durable-objects-code-update-mode immediate`); then
// 50% of calls to and from C (site 2) hang 3 s. When both A and B report C, undrain B under the 1% lead limit and drain
// C. Every group led on C at the drain is polled every second until a member on A or B leads it. Rows go to
// tests/m0carried/raw/b07-<run>.*.

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, groups: { type: "string", default: "200" }, "on-c": { type: "string", default: "80" },
  "no-deploy": { type: "boolean", default: false }, base: { type: "string" },
} });
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const URL_ = process.env.B07_URL ?? `https://graft-m3c-b07-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(`${process.env.HOME}/.m0carried.token`, "utf8").trim();
const run = opt.run!;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const events: any[] = [];
function log(what: string, extra: Record<string, unknown> = {}) {
  const e = { at: Date.now(), what, ...extra };
  events.push(e);
  appendFileSync(`${RAW}b07-${run}.events.jsonl`, JSON.stringify(e) + "\n");
  console.log(new Date(e.at).toISOString().slice(11, 19), what, JSON.stringify(extra).slice(0, 300));
}
async function edge<T = any>(path: string, body?: unknown, ms = 120_000): Promise<T> {
  for (let a = 0; ; a++) {
    try {
      const r = await fetch(URL_ + path, { method: body === undefined ? "GET" : "POST", body: body === undefined ? undefined : JSON.stringify(body),
        headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(ms) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 200)}`);
      return JSON.parse(t);
    } catch (e) {
      if (a >= 3) throw e;
      await sleep(1_000 * (a + 1));
    }
  }
}
const chunks = <T,>(xs: T[], n: number) => Array.from({ length: Math.ceil(xs.length / n) }, (_, i) => xs.slice(i * n, i * n + n));
const groupsPath = `${RAW}b07-${run}.groups.json`;

async function setup() {
  const n = Number(opt.groups), base = Number(opt.base ?? 7_000_000 + Math.floor(Math.random() * 900) * 1_000);
  const chains = Array.from({ length: n }, (_, i) => `repo:${base + i}`);
  // Leaders spread as M0's were: `--on-c` groups whose record's home is C, a sixth of the rest home B, the others A.
  // A group's leader returns to its home once it recovers, so home decides where load leaves it led.
  const nc = Number(opt["on-c"]), nb = Math.floor((n - nc) / 6);
  const homeOf = (i: number) => (i < nc ? 2 : i < nc + nb ? 1 : 0);
  const made: any[] = [];
  for (const h of [2, 1, 0]) {
    const mine = chains.filter((_, i) => homeOf(i) === h);
    for (const c of chunks(mine, 12)) made.push(...(await edge<any[]>("/create", { chains: c, home: h }, 300_000)).map((m) => ({ ...m, home: h })));
  }
  const ok = made.filter((m) => m.ok).map((m) => m.chain);
  log("created", { asked: n, ok: ok.length, byHome: count(made.filter((m) => m.ok).map((m) => m.home)), failed: made.filter((m) => !m.ok).slice(0, 5) });
  const onC = made.filter((m) => m.ok && m.home === 2).map((m) => m.chain);
  writeFileSync(groupsPath, JSON.stringify({ chains: ok, onC }));
  await lead(ok, onC);
}

/** Moves `onC`'s leadership to C, retrying until it leads there, and reports where every group is led. */
async function lead(all: string[], onC: string[]) {
  for (let i = 0; i < 4; i++) {
    const st = await status(onC, [0, 1, 2]);
    const left = onC.filter((g) => leaderOf(st.groups[g] ?? []) !== 2);
    if (!left.length) break;
    for (const c of chunks(left, 12)) await edge("/lead", { chains: c, to: 2 }, 300_000);
    await sleep(5_000);
  }
  const st = await status(all, [0, 1, 2]);
  log("led", { byLeader: count(Object.values(st.groups).map(leaderOf)) });
}

/** The site leading a group: a member answering `leader`, or else, as M0's runner counted it, the member holding
 *  `led_term` for the highest term any answering member reports (an idle leader whose lease lapsed resumes it on its
 *  next request). */
function leaderOf(ms: any[]): number | null {
  const l = ms.find((m) => m[1] === "leader");
  if (l) return l[0];
  const top = Math.max(...ms.filter((m) => m[1]).map((m) => m[2]));
  const r = ms.find((m) => m[1] && m[4] === top);
  return r ? r[0] : null;
}
function count(xs: (number | null)[]) {
  const o: Record<string, number> = {};
  for (const x of xs) o[String(x)] = (o[String(x)] ?? 0) + 1;
  return o;
}
async function status(chains: string[], sites: number[]): Promise<{ at: number; groups: Record<string, any[]> }> {
  // One invocation runs 6 member calls at once, so 4 chains of 3 members each answers in about two rounds.
  const parts: { at: number; groups: Record<string, any[]> }[] = [];
  const queue = chunks(chains, 4);
  await Promise.all(Array.from({ length: 10 }, async () => {
    for (let c = queue.shift(); c; c = queue.shift()) {
      parts.push(await edge<any>("/status", { chains: c, sites }, 60_000).catch((e) => { log("status failed", { error: String(e).slice(0, 160) }); return { at: Date.now(), groups: {} }; }));
    }
  }));
  return { at: Math.max(...parts.map((p) => p.at)), groups: Object.assign({}, ...parts.map((p) => p.groups)) };
}

function wrangler(args: string[]): string {
  return execFileSync("npx", ["--prefix", `${ROOT}../m0`, "wrangler", ...args], { cwd: `${ROOT}../../workers/site/.gen`, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

async function day() {
  const { chains, onC: toC } = JSON.parse(readFileSync(groupsPath, "utf8")) as { chains: string[]; onC: string[] };
  await edge("/fault", { off: true });
  await edge("/modes", { drained: [], limited: [] });
  const ops: any[] = [];
  let loading = true;
  // 20 invocations x 5 writers x 1 write/s = 100 writes/s, each invocation 15 s, observers alternating A and B.
  const load = Promise.all(Array.from({ length: 20 }, async (_, i) => {
    while (loading) {
      const r = await edge<any>("/bench", { chains, seconds: 15, workers: 5, rate: 1, observer: i % 2 }, 120_000).catch((e) => ({ error: String(e) }));
      if (r.error) { log("bench failed", { error: r.error.slice(0, 200) }); continue; }
      for (const [k, t, ms, ok] of r.ops) ops.push([chains[k], t, ms, ok]);
    }
  }));
  await sleep(20_000);
  // Under load, every group whose home is C should be led there; a site day before this one reset every member.
  await lead(chains, toC);
  await sleep(10_000);
  log("drain B");
  await edge("/modes", { drained: [1], limited: [] });
  await sleep(15_000);
  if (!opt["no-deploy"]) {
    const up = wrangler(["versions", "upload", "-c", "graft-m3c-site-b.jsonc", "--message", `B0-07 ${run}`]);
    const v = /Worker Version ID: ([0-9a-f-]+)/.exec(up)?.[1];
    const dep = JSON.parse(wrangler(["deployments", "status", "--name", "graft-m3c-site-b", "--json"]));
    const old = dep.versions?.[0]?.version_id;
    log("uploaded B", { v, old });
    wrangler(["versions", "deploy", `${v}@10`, `${old}@90`, "--name", "graft-m3c-site-b", "-y", "--message", `B0-07 ${run} 10%`,
      "--durable-objects-code-update-mode", "immediate"]);
    log("deployed B at 10%", { v, old });
    writeFileSync(`${RAW}b07-${run}.versions.json`, JSON.stringify({ v, old }));
  }
  await sleep(15_000);
  const before = await status(chains, [0, 1, 2]);
  const fault = await edge<any>("/fault", { site: 2, rate: 0.5, hang_ms: 3_000, for_ms: 3_600_000, after_ms: 3_000 });
  const grayFrom = fault.plan.from;
  log("gray fault set on C", { from: grayFrom, ledOnC: Object.values(before.groups).filter((m) => leaderOf(m) === 2).length });
  // Detection: both other observers over the last 30 s (only seconds since the fault began).
  let health: any = null, td = 0;
  while (Date.now() - grayFrom < 180_000) {
    await sleep(2_000);
    health = await edge<any[]>(`/health?since=${Math.max(grayFrom, Date.now() - 30_000)}`).catch(() => null);
    const toC = (o: number) => health?.find((h: any) => h.observer === o)?.rows?.find?.((r: any) => r.target === 2);
    const burn = [0, 1].every((o) => { const x = toC(o); return x && x.calls >= 30 && x.failed / x.calls >= 0.1; });
    if (burn) {
      log("C burning on A and B", { a: toC(0), b: toC(1), sinceGrayMs: Date.now() - grayFrom });
      await edge("/modes", { drained: [1], limited: [1] });
      await edge("/modes", { drained: [], limited: [1] });
      log("B undrained under the 1% lead limit");
      td = Date.now();
      await edge("/modes", { drained: [2], limited: [1] });
      log("C drained", { detectMs: td - grayFrom });
      break;
    }
  }
  if (!td) log("C never detected");
  const atDrain = td ? await status(chains, [0, 1, 2]) : before;
  const onC = Object.entries(atDrain.groups).filter(([, m]) => leaderOf(m) === 2).map(([g]) => g);
  log("led on C at the drain", { n: onC.length });
  const moved = new Map<string, number>();
  const snaps: any[] = [];
  while (td && moved.size < onC.length && Date.now() - td < 90_000) {
    const s = await status(onC.filter((g) => !moved.has(g)), [0, 1]);
    snaps.push(s);
    for (const [g, m] of Object.entries(s.groups)) {
      const l = leaderOf(m);
      if (l === 0 || l === 1) moved.set(g, s.at - td);
    }
    await sleep(1_000);
  }
  await sleep(10_000);
  loading = false;
  await load;
  const end = await status(chains, [0, 1, 2]);
  await edge("/fault", { off: true });
  await edge("/modes", { drained: [], limited: [] });
  log("fault off, undrained");
  // The day stops at 10% (the automatic drain preempts it, \`graft-ops\`'s rule). Put B back on one version: the old one.
  if (!opt["no-deploy"] && existsSync(`${RAW}b07-${run}.versions.json`)) {
    const { old } = JSON.parse(readFileSync(`${RAW}b07-${run}.versions.json`, "utf8"));
    wrangler(["versions", "deploy", `${old}@100`, "--name", "graft-m3c-site-b", "-y", "--message", `B0-07 ${run} rollback`,
      "--durable-objects-code-update-mode", "immediate"]);
    log("B back on its old version at 100%", { old });
  }
  // Health for the record, then the per-group report.
  const result = report(chains, onC, moved, td, grayFrom, ops, end, health, snaps.length);
  writeFileSync(`${RAW}b07-${run}.json`, JSON.stringify(result, null, 2));
  writeFileSync(`${RAW}b07-${run}.ops.jsonl`, ops.map((o) => JSON.stringify(o)).join("\n") + "\n");
  log("result", result.summary);
}

function report(chains: string[], onC: string[], moved: Map<string, number>, td: number, grayFrom: number, ops: any[], end: any, health: any, polls: number) {
  const lastBefore = new Map<string, number>(), firstAfter = new Map<string, number>();
  for (const [g, t, ms] of ops) {
    const done = t + ms;
    if (done <= td) lastBefore.set(g, Math.max(lastBefore.get(g) ?? 0, done));
    if (t >= td) firstAfter.set(g, Math.min(firstAfter.get(g) ?? Infinity, t));
  }
  const groups = onC.map((g) => {
    const m = moved.get(g) ?? null, idle = lastBefore.has(g) ? td - lastBefore.get(g)! : null;
    const active = idle !== null && idle <= 50_000;
    const sinceNext = m !== null && firstAfter.has(g) ? td + m - firstAfter.get(g)! : null;
    return { g, movedMs: m, idleMs: idle, active, sinceNextRequestMs: sinceNext,
      passWritten: m !== null && m <= 20_000, passGate: m !== null && (active ? m <= 20_000 : (m <= 20_000 || (sinceNext ?? Infinity) <= 20_000)) };
  });
  const ms = groups.map((x) => x.movedMs).filter((x): x is number => x !== null).sort((a, b) => a - b);
  const w = ops.filter(([, t]) => t >= grayFrom - 60_000);
  const lat = ops.map((o) => o[2]).sort((a, b) => a - b);
  const summary = {
    run, detectMs: td ? td - grayFrom : null, ledOnCAtDrain: onC.length, moved: ms.length,
    p50: ms[ms.length >> 1] ?? null, max: ms.at(-1) ?? null, over20s: ms.filter((x) => x > 20_000).length,
    active: groups.filter((x) => x.active).length, idle: groups.filter((x) => !x.active).length,
    gateAsWritten: `${groups.filter((x) => x.passWritten).length} of ${groups.length}`,
    gate: `${groups.filter((x) => x.passGate).length} of ${groups.length}`,
    writes: ops.length, failed: ops.filter((o) => !o[3]).length, writesAroundGray: w.length,
    p99: lat[Math.floor(lat.length * 0.99)] ?? null, max_ms: lat.at(-1) ?? null,
    endByLeader: count(Object.values(end.groups).map(leaderOf)), polls,
  };
  return { summary, groups, health };
}

if (positionals[0] === "setup") await setup();
else if (positionals[0] === "day") await day();
else if (positionals[0] === "probe") {
  const { chains } = JSON.parse(readFileSync(groupsPath, "utf8"));
  const st = await status(chains, [0, 1, 2]);
  const roles = count(Object.values(st.groups).flat().map((m: any) => m[1]));
  console.log(JSON.stringify({ n: Object.keys(st.groups).length, roles, byLeader: count(Object.values(st.groups).map(leaderOf)), sample: Object.entries(st.groups).slice(0, 2) }));
}
else throw new Error("setup | day");
