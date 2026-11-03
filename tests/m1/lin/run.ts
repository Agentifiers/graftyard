// The 24-hour linearizability rerun's driver (BM-1; B0-11's carried note), against graft-m1-lin-tools (lin.ts).
// Everything that matters runs in Cloudflare (load-client and chaos alarms), so this driver only starts the run,
// copies events and fault counters to tests/m1/lin/raw/ as they come, and finishes it. It keeps its state in
// raw/<run>.state.json and resumes from it, so it runs as an orb service that survives reboots.
//
//   node tests/m1/lin/run.ts start  --run NAME [--hours 24] [--per-colo 1] [--concurrency 4] [--rate 5] [--keys 6]
//   node tests/m1/lin/run.ts watch  --run NAME    # start or resume, then copy events and counters until the end
//   node tests/m1/lin/run.ts finish --run NAME    # wait for the end, stop load and chaos, collect, check, report
//
// The run is two orb services: `watch` and `finish`. Both resume after a reboot.
//
// `start` is idempotent: a run whose state file exists is resumed, never restarted. `finish` refuses to overwrite a
// finished run's report.

import { execFileSync } from "node:child_process";
import { appendFileSync, createReadStream, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { parseArgs } from "node:util";
import { check as ackCheck } from "./ackcheck.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, hours: { type: "string", default: "24" }, "per-colo": { type: "string", default: "1" },
  concurrency: { type: "string", default: "4" }, rate: { type: "string", default: "5" }, keys: { type: "string", default: "6" },
  colos: { type: "string", default: "ATL,LHR,SIN" },
} });
const cmd = positionals[0];
if (!opt.run || !cmd) throw new Error("usage: node tests/m1/lin/run.ts start|watch|finish --run NAME");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const URL_ = "https://graft-m1-lin-tools.<workers-subdomain>.workers.dev";
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const statePath = `${RAW}${opt.run}.state.json`;
const reportPath = `${RAW}${opt.run}.report.json`;
const log = (m: string) => { const l = `${new Date().toISOString()} ${m}`; console.log(l); appendFileSync(`${RAW}${opt.run}.driver.log`, l + "\n"); };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const HINT_OF: Record<string, string> = { ATL: "enam", EWR: "enam", IAD: "enam", ORD: "enam", LHR: "weur", AMS: "weur", CDG: "weur",
  SIN: "apac", NRT: "apac", HKG: "apac" };

type State = { run: string; chain: string; start: number; end: number; clients: Record<string, string[]>; spec: object; eventsAfter: number };

async function call<T = any>(path: string, body: unknown, ms = 120_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` },
        signal: AbortSignal.timeout(ms) });
      const text = await res.text();
      if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 200)}`);
      return JSON.parse(text) as T;
    } catch (e) {
      if (attempt >= 5) throw e;
      log(`${path} failed (${String(e).slice(0, 160)}); retrying`);
      await sleep(5_000 * (attempt + 1));
    }
  }
}

const readState = (): State | null => (existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : null);
const saveState = (s: State) => writeFileSync(statePath, JSON.stringify(s, null, 1));

/** Clients created with each colo's hint until `per` land in that colo. */
async function place(per: number): Promise<Record<string, string[]>> {
  const out: Record<string, string[]> = {};
  const stamp = Date.now().toString(36);
  for (const c of opt.colos!.split(",")) {
    out[c] = [];
    for (let batch = 0; out[c].length < per && batch < 8; batch++) {
      const names = Array.from({ length: 10 }, (_, i) => `lin-${c}-${stamp}-${batch}-${i}`);
      const landed = await call<string[]>("/clients/place", { hint: HINT_OF[c], names });
      names.forEach((n, i) => { if (landed[i] === c && out[c].length < per) out[c].push(n); });
    }
    log(`${c}: ${out[c].length} of ${per} pinned`);
    if (!out[c].length) throw new Error(`no client landed in ${c}`);
  }
  return out;
}

async function start(): Promise<State> {
  const have = readState();
  if (have) { log(`resuming ${have.run}, ends ${new Date(have.end).toISOString()}`); return have; }
  let h = 0;
  for (const ch of opt.run!) h = (h * 31 + ch.charCodeAt(0)) % 90_000;
  const chain = `repo:${900_000 + h}`;
  const created = await call("/create", { chain }, 180_000);
  if (!created.warm?.ok) throw new Error(`create ${chain}: ${JSON.stringify(created).slice(0, 400)}`);
  log(`created ${chain}: ${created.verdict}`);
  const clients = await place(Number(opt["per-colo"]));
  const startAt = Date.now();
  const end = startAt + Number(opt.hours) * 3_600_000;
  const spec = { run: opt.run, chain, concurrency: Number(opt.concurrency), rate: Number(opt.rate), end, keys: Number(opt.keys), readShare: 0.4 };
  const s: State = { run: opt.run!, chain, start: startAt, end, clients, spec, eventsAfter: 0 };
  saveState(s);
  const names = Object.values(clients).flat();
  log(`clients ${JSON.stringify(await call("/clients/start", { names, spec }))}`);
  log(`chaos ${JSON.stringify(await call("/chaos/start", { run: opt.run, chain, end: end - 10_000 }))}`);
  log(`started ${opt.run}: ${chain}, ${names.length} clients, ends ${new Date(end).toISOString()}`);
  return s;
}

/** Copies new chaos events to raw/<run>.events.jsonl and the isolates' latest counters to raw/<run>.isolates.json. */
async function copy(s: State): Promise<void> {
  for (;;) {
    const page = await call<{ rows: { seq: number }[] }>("/chaos/events", { run: s.run, after: s.eventsAfter });
    if (!page.rows.length) break;
    appendFileSync(`${RAW}${s.run}.events.jsonl`, page.rows.map((r) => JSON.stringify(r)).join("\n") + "\n");
    s.eventsAfter = page.rows[page.rows.length - 1].seq;
    saveState(s);
  }
  writeFileSync(`${RAW}${s.run}.isolates.json`, JSON.stringify(await call("/chaos/isolates", { run: s.run }), null, 1));
}

async function watch(): Promise<void> {
  const s = await start();
  while (Date.now() < s.end + 60_000) {
    try {
      await copy(s);
      const st = await call<any[]>("/clients/status", { names: Object.values(s.clients).flat() });
      log(`clients ${st.map((x) => `${x.colo} ${x.rows}/${x.failed}${x.running ? "" : " idle"}`).join("  ")}`);
    } catch (e) {
      log(`watch: ${String(e).slice(0, 200)}`);
    }
    await sleep(Math.min(300_000, Math.max(5_000, s.end + 60_000 - Date.now())));
  }
  log("watch: the run has ended; the finish service writes the report");
}

const pct = (xs: number[], p: number) => xs.length ? xs[Math.min(xs.length - 1, Math.floor(xs.length * p))] : null;

async function finish(): Promise<void> {
  const s = readState();
  if (!s) throw new Error(`no run ${opt.run}`);
  if (existsSync(reportPath)) { log(`${s.run} already has a report; not overwriting`); return; }
  // The clients stop by themselves at `end`; an operation in flight then has up to 30 s more.
  while (Date.now() < s.end + 90_000) await sleep(Math.min(600_000, s.end + 90_000 - Date.now()));
  const names = Object.values(s.clients).flat();
  log(`stopping load and chaos: ${JSON.stringify(await call("/clients/stop", { names }))} ${JSON.stringify(await call("/chaos/stop", { run: s.run }))}`);
  await copy(s);
  // Histories and clock rows, paged out of each client.
  const histPath = `${RAW}${s.run}.jsonl`;
  writeFileSync(histPath, "");
  const clockRows: { colo: string; at: number; phase: string; site: number; offset: number | null; rtt: number }[] = [];
  let ops = 0;
  for (const [colo, ns] of Object.entries(s.clients)) {
    for (const name of ns) {
      for (let after = 0; ;) {
        const page = await call<any[]>("/clients/history", { name, run: s.run, after });
        if (!page.length) break;
        appendFileSync(histPath, page.map((r) => JSON.stringify(r)).join("\n") + "\n");
        ops += page.length;
        after = page[page.length - 1].seq;
      }
      for (let after = 0; ;) {
        const page = await call<any[]>("/clients/clock", { name, after });
        if (!page.length) break;
        clockRows.push(...page.map((r) => ({ colo, ...r })));
        after = page[page.length - 1].seq;
      }
    }
  }
  writeFileSync(`${RAW}${s.run}.clock.jsonl`, clockRows.map((r) => JSON.stringify(r)).join("\n") + "\n");
  log(`collected ${ops} operations, ${clockRows.length} clock rows`);
  // Clock offsets. Each row is one colo's reading of one member's clock: offset = member now - midpoint of the call,
  // known to within half the call's round trip. Two colos' clocks differ by oA - oB on the same member in the same
  // 60 s bucket, known to within (rttA + rttB) / 2; per bucket the member with the smallest sum is used. A cross-colo
  // offset of up to `bound` is covered by widening every interval by bound / 2 on each side.
  const perColo: Record<string, object> = {};
  const median = (xs: number[]) => { const a = [...xs].sort((x, y) => x - y); return a[Math.floor(a.length / 2)]; };
  for (const c of [...new Set(clockRows.map((r) => r.colo))]) {
    const rs = clockRows.filter((r) => r.colo === c && r.offset !== null);
    const o = rs.map((r) => r.offset!).sort((a, b) => a - b), t = rs.map((r) => r.rtt).sort((a, b) => a - b);
    perColo[c] = { samples: rs.length, offsetP01: pct(o, 0.01), offsetMedian: median(o), offsetP99: pct(o, 0.99), rttMedian: median(t), rttP99: pct(t, 0.99) };
  }
  const best = new Map<string, Map<string, { offset: number; rtt: number }>>(); // bucket/site -> colo -> lowest-rtt sample
  for (const r of clockRows) {
    if (r.offset === null) continue;
    const k = `${Math.floor(r.at / 60_000)}/${r.site}`;
    const m = best.get(k) ?? new Map(); best.set(k, m);
    if (!m.has(r.colo) || m.get(r.colo).rtt > r.rtt) m.set(r.colo, { offset: r.offset, rtt: r.rtt });
  }
  const pairs: Record<string, { diff: number[]; bound: number[] }> = {};
  const buckets = [...new Set([...best.keys()].map((k) => k.split("/")[0]))];
  const colos = Object.keys(perColo).sort();
  for (const b of buckets) for (const a of colos) for (const c of colos) {
    if (a >= c) continue;
    let pick: { diff: number; err: number } | null = null;
    for (const site of [0, 1, 2]) {
      const m = best.get(`${b}/${site}`), x = m?.get(a), y = m?.get(c);
      if (!x || !y) continue;
      const err = (x.rtt + y.rtt) / 2;
      if (!pick || err < pick.err) pick = { diff: x.offset - y.offset, err };
    }
    if (!pick) continue;
    const p = (pairs[`${a}-${c}`] ??= { diff: [], bound: [] });
    p.diff.push(pick.diff); p.bound.push(Math.abs(pick.diff) + pick.err);
  }
  const pairStats = Object.fromEntries(Object.entries(pairs).map(([k, v]) => {
    const d = [...v.diff].sort((x, y) => x - y), bd = [...v.bound].sort((x, y) => x - y);
    return [k, { buckets: d.length, diffMin: d[0], diffMedian: median(d), diffMax: d[d.length - 1], boundP99: pct(bd, 0.99), boundMax: bd[bd.length - 1] }];
  }));
  const bound = Math.max(0, ...Object.values(pairStats).map((p) => p.boundMax ?? 0));
  const widen = Math.ceil(bound / 2);
  log(`clock: per colo ${JSON.stringify(perColo)} pairs ${JSON.stringify(pairStats)} bound ${bound} ms, widen ${widen} ms`);
  // Porcupine, segmented per key (B0-11's -seg), on the recorded clocks and widened by the measured bound.
  const porc = `${ROOT}../../m0/porcupine`;
  const bin = "/tmp/lin-m0check";
  if (!existsSync(bin)) execFileSync("go", ["build", "-o", bin, "."], { cwd: porc, stdio: "inherit" });
  // Split by key in one streaming pass: a 24 h history widened by history-prep.py passes Node's 512 MB string limit.
  const check = async (path: string) => {
    const parts = new Map<string, string[]>();
    for await (const l of createInterface({ input: createReadStream(path) })) {
      if (!l) continue;
      const k = JSON.parse(l).op.key as string;
      const p = parts.get(k) ?? []; parts.set(k, p); p.push(l);
      if (p.length >= 50_000) { appendFileSync(`${path}.${k}`, p.join("\n") + "\n"); p.length = 0; }
    }
    for (const [k, p] of parts) if (p.length) appendFileSync(`${path}.${k}`, p.join("\n") + "\n");
    const out: Record<string, unknown> = {};
    for (const k of [...parts.keys()].sort()) {
      const kp = `${path}.${k}`;
      try { out[k] = JSON.parse(execFileSync(bin, ["-seg", "2000", "-timeout", "20m", kp], { encoding: "utf8", maxBuffer: 1 << 26 })); }
      catch (e) { out[k] = JSON.parse((e as { stdout: string }).stdout || "{\"result\":\"error\"}"); }
      rmSync(kp);
    }
    return out;
  };
  // On the recorded clocks, with M0's 100 ms assumption, and with the measured bound.
  const recorded = await check(histPath);
  const widenedBy = async (w: number) => {
    const p = `${RAW}${s.run}.widened${w}.jsonl`;
    execFileSync("python3", [`${ROOT}../../m0/run/history-prep.py`, histPath, p, String(w)], { stdio: "inherit" });
    const r = await check(p);
    rmSync(p);
    return r;
  };
  const at100 = await widenedBy(100);
  const wide = await widenedBy(widen);
  // The member-side ack evidence of every acknowledged write, on the members' clocks.
  const acks = await ackCheck(histPath);
  log(`ack evidence: ${JSON.stringify({ result: acks.result, acked: acks.acked, withEvidence: acks.withEvidence, byRule: acks.byRule })}`);
  // Latency and counts in one streaming pass: a 24 h history passes Node's 512 MB string limit.
  const lat: number[] = [];
  let operations = 0, noReply = 0;
  for await (const l of createInterface({ input: createReadStream(histPath) })) {
    if (!l) continue;
    const r = JSON.parse(l);
    operations++;
    if (r.ok) lat.push(r.ret - r.call); else noReply++;
  }
  lat.sort((a, b) => a - b);
  const events = existsSync(`${RAW}${s.run}.events.jsonl`) ? readFileSync(`${RAW}${s.run}.events.jsonl`, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
  const count = (re: RegExp) => events.filter((e) => re.test(e.what)).length;
  const isolates = JSON.parse(readFileSync(`${RAW}${s.run}.isolates.json`, "utf8")) as { site: number; counts: Record<string, number>; rolls: number }[];
  const faultTotals: Record<string, number> = {};
  for (const i of isolates) for (const [k, v] of Object.entries(i.counts)) faultTotals[`site${i.site} ${k}`] = (faultTotals[`site${i.site} ${k}`] ?? 0) + v;
  const status = await call("/status", { chain: s.chain }).catch((e) => String(e));
  const report = {
    run: s.run, chain: s.chain, start: new Date(s.start).toISOString(), end: new Date(s.end).toISOString(), clients: s.clients, spec: s.spec,
    operations, noReply,
    latency: { p50: pct(lat, 0.5), p99: pct(lat, 0.99), max: lat.at(-1) },
    chaos: { resets: count(/^reset/), leaderResets: count(/^reset .*\(leader\)/), transfers: count(/^transfer \d/), abandoned: count(/^transfer \d.*abandon/) },
    isolates: isolates.length, faultTotals, clock: { perColo, pairs: pairStats, widenMs: widen },
    check: { recorded, widened100: at100, widenedMeasured: wide, ackEvidence: acks }, status,
  };
  writeFileSync(reportPath, JSON.stringify(report, null, 1));
  log(`report ${reportPath}: ${JSON.stringify({ ops: report.operations, recorded: Object.values(recorded).map((r: any) => r.result), at100: Object.values(at100).map((r: any) => r.result), measured: Object.values(wide).map((r: any) => r.result), ackEvidence: acks.result })}`);
}

if (cmd === "start") await start();
else if (cmd === "watch") await watch();
else if (cmd === "finish") await finish();
else throw new Error(`unknown command ${cmd}`);
