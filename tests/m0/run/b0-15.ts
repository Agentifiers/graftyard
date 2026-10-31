// B0-15, idle cost (build/BUILD-PLAN.md, M0-WP1; decisions/DECISIONS.md, "Milestone 0"). 100,000 idle groups and
// 1,000 control groups join on `Tri`, each member measuring its two peers once. Control members then serve one request
// an hour from their own alarms, and every member rechecks `Rtt` on the first request it serves in a new hour. After
// 24 hours across a UTC midnight, idle groups must show zero billed duration and zero `Rtt` rechecks, and the control
// groups must show both.
//
//   node run/b0-15.ts setup  [--groups 100000] [--controls 1000] [--batch 100] [--parallel 32]
//   node run/b0-15.ts check            # Cloudflare analytics for the window so far, by class
//   node run/b0-15.ts final            # after the window: analytics, a report from every member, verdict, stop controls

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { SITE_KEYS, SITES, type MemberClass, type MemberRef, type MemberReport } from "../src/sites.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  groups: { type: "string", default: "100000" }, controls: { type: "string", default: "1000" },
  batch: { type: "string", default: "100" }, parallel: { type: "string", default: "32" },
  run: { type: "string" }, edge: { type: "string", default: "https://graft-m0-edge.<workers-subdomain>.workers.dev" },
  force: { type: "boolean", default: false },
} });
const BATCH = Number(opt.batch), PARALLEL = Number(opt.parallel);
const TOKEN = readFileSync(new URL("../.m0-token", import.meta.url), "utf8").trim();
const STATE = new URL("../results/raw/b0-15-state.json", import.meta.url);
const HOUR = 3_600_000, WINDOW = 24 * HOUR, SETTLE = 10 * 60_000;
const { CLOUDFLARE_API_TOKEN: CF_TOKEN, CLOUDFLARE_ACCOUNT_ID: ACCOUNT } = process.env;

interface State {
  run: string; groups: number; controls: number; namespaces: Record<string, MemberClass>;
  setupStart?: string; setupEnd?: string; windowStart?: string; windowEnd?: string;
  joinFailures?: number; controlStartFailures?: number;
}
const save = (s: State) => { mkdirSync(new URL("../results/raw/", import.meta.url), { recursive: true }); writeFileSync(STATE, JSON.stringify(s, null, 1)); };
const load = (): State => JSON.parse(readFileSync(STATE, "utf8"));

type Settled<T> = { ok: true; value: T } | { ok: false; error: string };
async function edge<T>(path: string, body: unknown): Promise<Settled<T>[]> {
  for (let attempt = 1; ; attempt++) {
    try {
      const res = await fetch(opt.edge + path, { method: "POST", body: JSON.stringify(body),
        headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" } });
      if (res.ok) return ((await res.json()) as { results: Settled<T>[] }).results;
      if (attempt >= 5) throw new Error(`${path}: HTTP ${res.status} ${(await res.text()).slice(0, 300)}`);
    } catch (e) { if (attempt >= 5) throw e; }
    await new Promise((r) => setTimeout(r, 1000 * 2 ** attempt));
  }
}

/** Runs `items` through `path` in batches, retrying only the items that failed, up to three rounds. */
async function drive<I, T>(label: string, items: I[], path: string, body: (b: I[]) => unknown): Promise<{ ok: T[]; failed: string[] }> {
  let pending = items; const ok: T[] = []; let failed: string[] = [];
  for (let round = 1; round <= 3 && pending.length; round++) {
    const batches: I[][] = [];
    for (let i = 0; i < pending.length; i += BATCH) batches.push(pending.slice(i, i + BATCH));
    const retry: I[] = []; failed = [];
    let next = 0, done = 0; const t0 = Date.now();
    await Promise.all(Array.from({ length: PARALLEL }, async () => {
      while (next < batches.length) {
        const b = batches[next++];
        const res = await edge<T>(path, body(b));
        res.forEach((r, i) => { if (r.ok) ok.push(r.value); else { retry.push(b[i]); failed.push(r.error); } });
        if (++done % 50 === 0 || done === batches.length)
          console.error(`${label} round ${round}: ${done}/${batches.length} batches, ${retry.length} failed, ${Math.round((Date.now() - t0) / 1000)} s`);
      }
    }));
    pending = retry;
  }
  return { ok, failed };
}

function groupsOf(run: string, n: number, cls: "idle" | "control", tag: string): MemberRef[][] {
  return Array.from({ length: n }, (_, g) => SITE_KEYS.map((k, slot) =>
    ({ site: k, name: `b015-${run}-${tag}${g}#1#f${slot}`, hint: SITES[k].hint, cls })));
}
const joinJobs = (groups: MemberRef[][]) => groups.flatMap((g) => g.map((m) => ({ member: m, peers: g.filter((p) => p !== m) })));

async function cf<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`https://api.cloudflare.com/client/v4${path}`, { ...init,
    headers: { authorization: `Bearer ${CF_TOKEN}`, "content-type": "application/json" } });
  const j = await res.json() as { success?: boolean; errors?: unknown; result?: T; data?: T };
  if (j.errors && (j.success === false || Array.isArray(j.errors) && j.errors.length)) throw new Error(JSON.stringify(j.errors));
  return (j.result ?? j.data) as T;
}

async function namespaces(): Promise<Record<string, MemberClass>> {
  const list = await cf<{ id: string; script: string; class: string }[]>(`/accounts/${ACCOUNT}/workers/durable_objects/namespaces`);
  const byClass: Record<string, MemberClass> = { Member: "member", IdleMember: "idle", ControlMember: "control" };
  return Object.fromEntries(list.filter((n) => n.script.startsWith("graft-site-") && byClass[n.class]).map((n) => [n.id, byClass[n.class]]));
}

/** Requests, active time, and billed duration per class over [start, end), from Cloudflare's own analytics. */
async function usage(ns: Record<string, MemberClass>, start: string, end: string) {
  const query = `query($a:String!,$s:Time!,$e:Time!,$n:[String!]){viewer{accounts(filter:{accountTag:$a}){
    inv: durableObjectsInvocationsAdaptiveGroups(limit:1000, filter:{datetime_geq:$s, datetime_lt:$e, namespaceId_in:$n}){ sum{requests} dimensions{namespaceId} }
    per: durableObjectsPeriodicGroups(limit:1000, filter:{datetime_geq:$s, datetime_lt:$e, namespaceId_in:$n}){ sum{activeTime duration cpuTime rowsWritten} dimensions{namespaceId} }
  }}}`;
  const data = await cf<{ viewer: { accounts: { inv: any[]; per: any[] }[] } }>("/graphql", { method: "POST",
    body: JSON.stringify({ query, variables: { a: ACCOUNT, s: start, e: end, n: Object.keys(ns) } }) });
  const acc = data.viewer.accounts[0];
  const out: Record<string, { requests: number; activeTimeS: number; durationGBs: number; cpuMs: number; rowsWritten: number }> = {};
  for (const cls of new Set(Object.values(ns))) out[cls] = { requests: 0, activeTimeS: 0, durationGBs: 0, cpuMs: 0, rowsWritten: 0 };
  for (const r of acc.inv) out[ns[r.dimensions.namespaceId]].requests += r.sum.requests;
  for (const r of acc.per) {
    const o = out[ns[r.dimensions.namespaceId]];
    o.activeTimeS += r.sum.activeTime / 1e6; o.durationGBs += r.sum.duration; o.cpuMs += r.sum.cpuTime / 1e3; o.rowsWritten += r.sum.rowsWritten;
  }
  return out;
}

const cmd = positionals[0];
if (cmd === "setup") {
  if (existsSync(STATE) && !opt.force) throw new Error("a B0-15 run already exists; pass --force to start another");
  const run = opt.run ?? new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13);
  const state: State = { run, groups: Number(opt.groups), controls: Number(opt.controls), namespaces: await namespaces(), setupStart: new Date().toISOString() };
  save(state);
  const controls = groupsOf(run, state.controls, "control", "c");
  const c = await drive("control join", joinJobs(controls), "/join", (b) => ({ jobs: b }));
  // Spread the controls' first alarms over the first hour, so their load is flat and every hour boundary is crossed.
  const cm = controls.flat();
  const offsets = new Map(cm.map((m, i) => [m.name, 60_000 + Math.floor((i / cm.length) * HOUR)]));
  const s = await drive<MemberRef, void>("control start", cm, "/control/start", (b) => ({ members: b, offsetsMs: b.map((m) => offsets.get(m.name)) }));
  const i = await drive("idle join", joinJobs(groupsOf(run, state.groups, "idle", "g")), "/join", (b) => ({ jobs: b }));
  state.setupEnd = new Date().toISOString();
  state.windowStart = new Date(Date.now() + SETTLE).toISOString();
  state.windowEnd = new Date(Date.now() + SETTLE + WINDOW).toISOString();
  state.joinFailures = c.failed.length + i.failed.length;
  state.controlStartFailures = s.failed.length;
  save(state);
  console.log(JSON.stringify({ ...state, namespaces: undefined, failureSample: [...c.failed, ...i.failed, ...s.failed].slice(0, 5) }, null, 2));
} else if (cmd === "check") {
  const st = load();
  const end = new Date(Math.min(Date.now(), Date.parse(st.windowEnd!))).toISOString();
  console.log(JSON.stringify({ window: [st.windowStart, end], setupToWindow: await usage(st.namespaces, st.setupEnd!, st.windowStart!),
    usage: Date.parse(end) > Date.parse(st.windowStart!) ? await usage(st.namespaces, st.windowStart!, end) : "window not started" }, null, 2));
} else if (cmd === "final") {
  const st = load();
  if (Date.now() < Date.parse(st.windowEnd!) && !opt.force) throw new Error(`window ends ${st.windowEnd}`);
  // Analytics first, for the window only; the report pass below wakes every member after the window has closed.
  const u = await usage(st.namespaces, st.windowStart!, st.windowEnd!);
  const idle = groupsOf(st.run, st.groups, "idle", "g").flat(), control = groupsOf(st.run, st.controls, "control", "c").flat();
  const ri = await drive<MemberRef, MemberReport>("idle report", idle, "/report", (b) => ({ members: b }));
  const rc = await drive<MemberRef, MemberReport>("control report", control, "/report", (b) => ({ members: b }));
  await drive<MemberRef, void>("control stop", control, "/control/stop", (b) => ({ members: b }));
  // A recheck is a measurement after the window opened. `rttChecks` alone would also count a join that setup retried,
  // and a join still in flight when setup returned can finish just after `setupEnd`. Controls keep serving hourly until
  // `final` stops them, so their latest measurement may fall after the window.
  const windowStart = Date.parse(st.windowStart!);
  const idleRechecked = ri.ok.filter((r) => r.lastMeasuredAt > windowStart || r.served > 0);
  const idleNeverJoined = ri.ok.filter((r) => r.rttChecks === 0);
  const ctrlChecks = rc.ok.map((r) => r.rttChecks - 1).sort((a, b) => a - b);
  const ctrlRechecked = (r: MemberReport) => r.served > 0 && r.lastMeasuredAt > windowStart;
  const idleZero = u.idle.durationGBs === 0 && u.idle.activeTimeS === 0 && u.idle.requests === 0 && idleRechecked.length === 0;
  const controlShows = u.control.durationGBs > 0 && rc.ok.length === control.length && rc.ok.every(ctrlRechecked);
  const summary = {
    run: st.run, window: [st.windowStart, st.windowEnd], groups: st.groups, controls: st.controls, usage: u,
    idle: { reported: ri.ok.length, of: idle.length, reportFailures: ri.failed.length, rechecked: idleRechecked.length, neverJoined: idleNeverJoined.length,
            sample: idleRechecked.slice(0, 5) },
    control: { reported: rc.ok.length, of: control.length, rechecksPerMember: { min: ctrlChecks[0], p50: ctrlChecks[ctrlChecks.length >> 1], max: ctrlChecks.at(-1) },
               withoutRecheck: rc.ok.filter((r) => !ctrlRechecked(r)).length },
    verdict: idleZero && controlShows ? "Pass" : !controlShows ? "Fail: a control group shows no duration or no recheck, so the meter is not proven" : "Fail: idle groups billed or rechecked",
  };
  writeFileSync(new URL(`../results/b0-15-${st.run}.json`, import.meta.url), JSON.stringify(summary, null, 2));
  console.log(JSON.stringify(summary, null, 2));
} else {
  throw new Error("usage: node run/b0-15.ts setup|check|final");
}
