// Chaos run for the M0-WP2 Raft prototype, checked with Porcupine: the short form of B0-11. Clients run register
// operations through the edge while the run drops and fails member calls at random, resets members, and transfers
// leadership, some transfers abandoned mid-way. Timestamps are taken here, around each client call, so the history is
// valid for a linearizability check whatever the edge does inside.
//
//   node run/raft-chaos.ts [--url http://127.0.0.1:8799 --token local] [--seconds 120] [--clients 8] [--keys 6]
//                          [--drop 0.05] [--fail 0.05] [--resets 1] [--transfers 1] [--group chaos-<run>]
//   --slow-every S [--slow-for 60 --slow-rate 0.3 --slow-hang 1000]: every S seconds (randomized x1 to x1.5), make
//   --slow-rate of the leader's site's storage writes confirm --slow-hang ms late for --slow-for seconds (a gray store,
//   M0-WP4), so `STEP_DOWN` trips during the run. With --slow-alone, the call faults pause during each slow period:
//   at 5% failed calls plus 5% dropped replies a leader's commit-error rate already passes `STEP_DOWN`'s 5%, and
//   `checkHealth` names commit errors first, so the confirm rule would never be the one that trips. --slow-lead S also
//   pauses them S seconds before each slow period: under 5% call faults every leader trips on commit errors within
//   seconds, so all three sites sit inside RESET_QUARANTINE and a confirm trip is always held (wp4-cf-t1); a lead of
//   just over RESET_QUARANTINE (300 s) lets the slow leader actually transfer. The members' logs are copied every 15 s and before each reset (a reset
//   empties the in-memory log) to raft-chaos-<run>.members.jsonl, so stalls can be traced after the run.
//   --remote: the load comes from graft-m0-tools' pinned clients (ATL, LHR, SIN) through their RAFT service binding,
//   not from this machine, so a long run does not depend on the orb staying up; this machine only drives the chaos and
//   collects the clients' histories at the end (`node run/tools.ts collect --run <run>` also works after a restart).

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import type { Op, Out } from "../src/raft/types.ts";

const ROOT = new URL("../", import.meta.url).pathname;
const { values: opt } = parseArgs({ options: {
  url: { type: "string", default: "https://graft-raft-edge.<workers-subdomain>.workers.dev" },
  token: { type: "string" }, seconds: { type: "string", default: "120" }, clients: { type: "string", default: "8" },
  keys: { type: "string", default: "6" }, drop: { type: "string", default: "0.05" }, fail: { type: "string", default: "0.05" },
  resets: { type: "string", default: "1" }, every: { type: "string", default: "8" }, transfers: { type: "string", default: "1" }, site: { type: "string" }, remote: { type: "boolean", default: false }, resume: { type: "boolean", default: false },
  learners: { type: "string", default: "0" },
  "slow-every": { type: "string", default: "0" }, "slow-for": { type: "string", default: "60" }, "slow-rate": { type: "string", default: "0.3" },
  "slow-hang": { type: "string", default: "1000" }, "slow-alone": { type: "boolean", default: false }, "slow-lead": { type: "string", default: "0" }, "remote-chaos": { type: "boolean", default: false },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) }, group: { type: "string" },
} });
const TOKEN = opt.token ?? readFileSync(`${ROOT}.m0-token`, "utf8").trim();
// A run whose summary exists is finished: a supervised driver restarted after it ends must not redo or overwrite it.
if (existsSync(`${ROOT}results/raw/raft-chaos-${opt.run}.json`)) {
  console.error(`run ${opt.run} already has results; refusing to overwrite`);
  process.exit(0);
}
const group = opt.group ?? `chaos-${opt.run}`;
// --resume (with --remote and the same --run): the pinned clients are still running a run whose driver died, as on an
// orb restart, so skip creating the group and starting clients, and keep the chaos going until the clients' end.
let end = Date.now() + Number(opt.seconds) * 1000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function post<T>(path: string, body: unknown, timeoutMs = 40_000): Promise<T> {
  const res = await fetch(opt.url + path, { method: "POST", body: JSON.stringify({ group, ...(body as object) }),
    headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
  const text = await res.text();
  if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 200)}`);
  return JSON.parse(text) as T;
}

type Row = { client: number; call: number; ret: number; op: Op; ok: boolean; out?: Out; error?: string };
const history: Row[] = [];
const events: { at: number; what: string; result: string }[] = [];
// Every chaos event is also appended to a sidecar as it happens, so a driver restarted mid-run keeps the earlier ones.
const eventsPath = `${ROOT}results/raw/raft-chaos-${opt.run}.events.jsonl`;
const logEvent = (e: { at: number; what: string; result: string }) => { events.push(e); appendFileSync(eventsPath, JSON.stringify(e) + "\n"); };
// History rows as they complete, so a driver that dies mid-run leaves its history; the final file replaces it.
const historyPartial = `${ROOT}results/raw/raft-chaos-${opt.run}.partial.jsonl`;
const record = (r: Row) => { history.push(r); appendFileSync(historyPartial, JSON.stringify(r) + "\n"); };

// The members' logs, copied before a reset or the 300-line ring drops them. Each new line is written once.
const membersPath = `${ROOT}results/raw/raft-chaos-${opt.run}.members.jsonl`;
const seen = new Map<string, Set<string>>();
async function harvest(): Promise<void> {
  const ms = await post<{ site?: string; log?: string[] }[]>("/stats", { reset: false }, 30_000).catch(() => []);
  for (const m of ms) {
    if (!m.site || !m.log) continue;
    const set = seen.get(m.site) ?? new Set<string>(); seen.set(m.site, set);
    const fresh = m.log.filter((l) => !set.has(l));
    for (const l of fresh) set.add(l);
    if (fresh.length) appendFileSync(membersPath, fresh.map((line) => JSON.stringify({ site: m.site, line })).join("\n") + "\n");
  }
}

async function client(id: number): Promise<void> {
  let i = 0;
  const lastRead = new Map<string, string | null>();
  while (Date.now() < end) {
    const key = `k${Math.floor(Math.random() * Number(opt.keys))}`;
    const value = `${id}.${i++}`;
    const r = Math.random();
    const op: Op = r < 0.4 ? { kind: "get", key } : r < 0.75 ? { kind: "put", key, value }
      : { kind: "cas", key, expect: lastRead.get(key) ?? null, value };
    const call = Date.now();
    try {
      // With --learners N, half the gets go to learner slot 0..N-1 (M0-WP3), so the check covers learner reads.
      const learner = op.kind === "get" && Number(opt.learners) && Math.random() < 0.5 ? Math.floor(Math.random() * Number(opt.learners)) : undefined;
      const { out } = await post<{ out: Out }>("/op", { op, learner });
      record({ client: id, call, ret: Date.now(), op, ok: true, out });
      if (op.kind === "get") lastRead.set(key, out.value ?? null);
    } catch (e) {
      record({ client: id, call, ret: Date.now(), op, ok: false, error: String(e).slice(0, 160) });
    }
    await sleep(20 + Math.random() * 60);
  }
}

/** Faults on member-to-member calls only (`>`-labelled calls from members, not the edge), so clients still reach the
 *  group and the protocol, not the client path, is what gets stressed. */
async function chaos(): Promise<void> {
  const plan = { rules: [
    ...(Number(opt.drop) ? [{ target: "call", mode: "drop-reply", rate: Number(opt.drop), match: opt.site ? `>${opt.site}:append` : ":append" }] : []),
    // Member-to-member only: an empty match would also fail the edge's calls and the remote load clients' `RAFT:op`.
    // With --site, both directions between that site and the others, as a gray site breaks both.
    ...(Number(opt.fail) ? (opt.site ? ["a", "b", "c"].filter((x) => x !== opt.site).flatMap((x) => [`${x}>${opt.site}:`, `${opt.site}>${x}:`]) : ["a>", "b>", "c>"])
      .map((match) => ({ target: "call", mode: "error", rate: Number(opt.fail), match })) : []),
  ] };
  await post("/faults", { plan });
  logEvent({ at: Date.now(), what: "faults", result: JSON.stringify(plan) });
  const sites = ["a", "b", "c"];
  let running = true;
  const harvester = (async () => { while (running) { await harvest(); await sleep(15_000); } })();
  // Slow-storage periods on the leader's site: the fault plan is the base plan plus one slow-reply write rule, then the
  // base plan again. Not `until`, which would end the base plan's call faults with it.
  const slower = (async () => {
    const every = Number(opt["slow-every"]) * 1000;
    if (!every) return;
    for (;;) {
      await sleep(every * (1 + Math.random() * 0.5));
      if (Date.now() > end - 10_000 - (Number(opt["slow-for"]) + Number(opt["slow-lead"])) * 1000) return;
      if (Number(opt["slow-lead"])) {
        const r = await post("/faults", { plan: { rules: [] } }).then(() => "ok", (e) => `plan not set: ${String(e).slice(0, 120)}`);
        logEvent({ at: Date.now(), what: "call faults paused", result: r });
        await sleep(Number(opt["slow-lead"]) * 1000);
      }
      const st = await post<{ site: string; role: string }[]>("/status", {}).catch(() => []);
      const site = st.find((x) => x.role === "leader")?.site ?? sites[Math.floor(Math.random() * 3)];
      const rule = { target: "write", mode: "slow-reply", rate: Number(opt["slow-rate"]), match: `${site}:`, hangMs: Number(opt["slow-hang"]) };
      const set = await post<{ plan: unknown }>("/faults", { plan: { rules: [...(opt["slow-alone"] ? [] : plan.rules), rule] } })
        .then((r) => JSON.stringify(r.plan), (e) => `plan not set: ${String(e).slice(0, 120)}`);
      logEvent({ at: Date.now(), what: `slow storage on ${site}`, result: set });
      await sleep(Number(opt["slow-for"]) * 1000);
      const back = await post("/faults", { plan }).then(() => "ok", (e) => `plan not set: ${String(e).slice(0, 120)}`);
      logEvent({ at: Date.now(), what: `slow storage on ${site} ended`, result: back });
    }
  })();
  while (Date.now() < end - 10_000) {
    await sleep(Number(opt.every ?? 8) * 1000 * (1 + Math.random()));
    const roll = Math.random();
    try {
      if (roll < 0.6 && Number(opt.resets)) {
        // Half the resets target the leader, the case where unconfirmed tail entries are lost.
        const st = await post<{ site: string; role: string }[]>("/status", {}).catch(() => []);
        const leader = st.find((x) => x.role === "leader")?.site;
        const s = leader && Math.random() < 0.5 ? leader : sites[Math.floor(Math.random() * 3)];
        await harvest();
        await post("/reset", { sites: [s] });
        logEvent({ at: Date.now(), what: `reset ${s}`, result: "ok" });
      } else if (Number(opt.transfers)) {
        const to = sites[Math.floor(Math.random() * 3)];
        const abandon = Math.random() < 0.3;
        const r = await post<{ from: string; result: string }>("/transfer", { to, abandon });
        logEvent({ at: Date.now(), what: `transfer ${r.from}->${to}${abandon ? " (abandon)" : ""}`, result: r.result });
      }
    } catch (e) {
      logEvent({ at: Date.now(), what: "chaos step", result: String(e).slice(0, 120) });
    }
  }
  await slower;
  await post("/faults", { plan: { rules: [] } });
  running = false;
  await harvester;
  await harvest();
}

const TOOLS = "https://graft-m0-tools.<workers-subdomain>.workers.dev";
async function tools<T>(path: string, body: unknown): Promise<T> {
  const res = await fetch(TOOLS + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` } });
  if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
  return res.json() as Promise<T>;
}
// --remote-chaos: the chaos loop runs as an alarm in graft-m0-chaos (src/chaos.ts), so resets and transfers go on
// while this orb is asleep. This driver starts it once, then only copies its events into the events file. The fault
// plan (dropped and failed member calls) lives in graft-m0-tools and is set once, so it persists either way.
const CHAOS = "https://graft-m0-chaos.<workers-subdomain>.workers.dev";
async function remoteChaos(): Promise<void> {
  const call = async <T>(path: string, body: object) => {
    const res = await fetch(CHAOS + path, { method: "POST", body: JSON.stringify({ run: opt.run, ...body }), headers: { authorization: `Bearer ${TOKEN}` },
      signal: AbortSignal.timeout(30_000) });
    if (!res.ok) throw new Error(`${path}: ${res.status} ${await res.text()}`);
    return res.json() as Promise<T>;
  };
  const seqPath = `${ROOT}results/raw/raft-chaos-${opt.run}.remote-seq`;
  let after = existsSync(seqPath) ? Number(readFileSync(seqPath, "utf8")) : 0;
  const first = await call<{ running: boolean }>("/events", { after: 1e12 });
  if (!first.running) {
    const r = await call("/start", { url: opt.url, group, end, every: Number(opt.every) });
    logEvent({ at: Date.now(), what: "remote chaos driver", result: JSON.stringify(r) });
  }
  for (;;) {
    const page = await call<{ running: boolean; rows: { seq: number; at: number; what: string; result: string }[] }>("/events", { after })
      .catch(() => ({ running: true, rows: [] })); // a hung or failed poll retries after the sleep below
    for (const r of page.rows) { logEvent({ at: r.at, what: r.what, result: r.result }); after = r.seq; }
    writeFileSync(seqPath, String(after));
    // `running` reads the next alarm, which is unset while an alarm runs, so it can read false mid-run: stop only
    // once the run's end has passed as well.
    if (!page.running && !page.rows.length && Date.now() > end) break;
    if (!page.rows.length) await sleep(30_000);
  }
}
const pinned = Object.values(JSON.parse(readFileSync(`${ROOT}results/raw/tools-clients.json`, "utf8")) as Record<string, string[]>).flat();
if (opt.remote) {
  if (opt.resume) {
    end = Math.max(...(await tools<{ end: number }[]>("/clients/status", { names: pinned })).map((x) => x.end));
    logEvent({ at: Date.now(), what: "resumed driver", result: new Date(end).toISOString() });
  } else {
  await post("/op", { op: { kind: "put", key: "warm", value: "0" } }); // creates the group with A leading
  // Each pinned client runs `--clients` logical clients at about 12 ops/s each, as the local clients do (20 to 80 ms
  // between calls plus the call); its client ids start at i * 100, so they never collide.
  const spec = { run: opt.run, target: "RAFT", service: true, object: group, concurrency: Number(opt.clients), rate: 5,
    durationMs: Number(opt.seconds) * 1000, keys: Number(opt.keys), readShare: 0.4, callTimeoutMs: 45_000 };
  logEvent({ at: Date.now(), what: "remote clients", result: JSON.stringify(await tools("/clients/start", { names: pinned, spec })) });
  }
  // A resumed driver joins a remote chaos driver already running for this run, so a plain `--resume` (as the B0-11
  // schedule runs it) never starts a second chaos loop beside it.
  const remoteUp = await fetch(CHAOS + "/events", { method: "POST", body: JSON.stringify({ run: opt.run, after: 1e12 }),
    headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(30_000) }).then((r) => r.json() as Promise<{ running: boolean }>).then((r) => r.running).catch(() => false);
  // A run that has used the remote driver keeps using it: `running` can read false mid-alarm, so the seq file decides.
  const usedRemote = existsSync(`${ROOT}results/raw/raft-chaos-${opt.run}.remote-seq`);
  if (opt["remote-chaos"] || remoteUp || usedRemote) await remoteChaos(); else await chaos();
  // `running` reads the next alarm, which is unset while an alarm runs, so also wait past the end plus one call timeout.
  const doneAt = Math.max(Date.now(), end) + 10_000 + 45_000 + 5_000;
  for (;;) {
    await sleep(10_000);
    const st = await tools<{ running: boolean }[]>("/clients/status", { names: pinned });
    if (Date.now() > doneAt && st.every((x) => !x.running)) break;
  }
  for (const name of pinned) {
    for (let after = 0; ;) {
      const page = await tools<(Row & { seq: number })[]>("/clients/history", { name, run: opt.run, after });
      if (!page.length) break;
      history.push(...page);
      after = page[page.length - 1].seq;
    }
  }
} else {
  await Promise.all([...Array.from({ length: Number(opt.clients) }, (_, i) => client(i)), chaos()]);
}
const status = await post("/status", {}).catch((e) => String(e));
mkdirSync(`${ROOT}results/raw`, { recursive: true });
const path = `${ROOT}results/raw/raft-chaos-${opt.run}.jsonl`;
// In batches: a 48-hour history is longer than the largest string V8 can join.
writeFileSync(path, "");
for (let i = 0; i < history.length; i += 50_000) appendFileSync(path, history.slice(i, i + 50_000).map((r) => JSON.stringify(r) + "\n").join(""));
let check: Record<string, unknown>;
try {
  check = JSON.parse(execFileSync("go", ["run", ".", path], { cwd: `${ROOT}porcupine`, encoding: "utf8" }));
} catch (e) {
  check = JSON.parse((e as { stdout: string }).stdout);
}
const lat = history.filter((r) => r.ok).map((r) => r.ret - r.call).sort((a, b) => a - b);
const summary = { run: opt.run, url: opt.url, group, seconds: Number(opt.seconds), clients: Number(opt.clients), check,
  ok: history.filter((r) => r.ok).length, failed: history.filter((r) => !r.ok).length,
  p50: lat[Math.floor(lat.length * 0.5)], p99: lat[Math.floor(lat.length * 0.99)], max: lat.at(-1),
  failures: Object.entries(history.filter((r) => !r.ok).reduce((m, r) => { const k = r.error!.slice(0, 80); m[k] = (m[k] ?? 0) + 1; return m; }, {} as Record<string, number>)),
  events: existsSync(eventsPath) ? readFileSync(eventsPath, "utf8").trim().split("\n").map((l) => JSON.parse(l)) : events, status };
writeFileSync(`${ROOT}results/raw/raft-chaos-${opt.run}.json`, JSON.stringify(summary, null, 1));
console.log(JSON.stringify(summary, null, 1));
