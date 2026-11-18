// B0-34 runner (build/BUILD-PLAN.md, M0-WP9): deploys `graft-wp9-git` (src/wp9/git.ts, image wp9/git/Dockerfile),
// runs the phased replay, collects, and stops every container.
//
//   node run/wp9-git.ts deploy
//   node run/wp9-git.ts run [--run X] [--seconds 1200] [--clients 12] [--mode stock|compose]
//   node run/wp9-git.ts stop --run X
//   node run/wp9-git.ts delete
//
// The bucket `graft-wp9-git` holds the networks, uploaded from this orb by run/wp9-gitprep.ts: `net/linux/` (a bare
// clone of torvalds/linux; the second pass used 31 packs of at most 250 MB, the third one pack in 250 MB byte parts)
// and `net/git/` (git/git),
// each with `refs`, the real commits the agents push (`commits`), real pulls the merge queue merges (`merges`), and the
// tip's depth-1 pack and object list (`base.pack`, `base.objs`) for compose misses.
//
// Phases (seconds from the start of load):
//   0     the monorepo `linux` (owner mono) warm on sites a and b, journey 9's git work on it
//   420   site a killed: every container of the site destroyed; spare site c becomes warm
//   600   a second network `gitnet` (owner other) turns hot; the API guard counts any Cloudflare API call
//   840   a hostile owner makes three of its networks hot (`hostile1`..`3`)
//   end   collect

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import type { DriveSpec, Stream } from "../src/wp9/git.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(4, 12) },
  seconds: { type: "string", default: "1200" }, clients: { type: "string", default: "12" }, mode: { type: "string", default: "stock" },
  // Third pass: --alike sizes both warm sites from the larger NetworkLoad; --up/--down set hysteresis in seconds
  // (accelerated 30 and 120 by default; the design's 180 and 3600); --tip-per-s moves the tip (10 merges a minute).
  alike: { type: "boolean", default: false }, up: { type: "string", default: "30" }, down: { type: "string", default: "120" },
  "tip-per-s": { type: "string", default: "0" },
  // --j9-only runs phase 1 alone (no kill, no second or hostile networks) at --scale times journey 9's linux rates.
  "j9-only": { type: "boolean", default: false }, scale: { type: "string", default: "1" } } });
const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = `https://graft-wp9-git.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const DATA = "/home/user/wp9-data";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const log = (...a: unknown[]) => console.log(new Date().toISOString().slice(11, 19), ...a);

function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });
}
async function call<T>(path: string, body: unknown, timeoutMs = 120_000): Promise<T> {
  const r = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
  const t = await r.text();
  if (!r.ok) throw new Error(`${path}: ${r.status} ${t.slice(0, 300)}`);
  return JSON.parse(t) as T;
}

function deploy(): void {
  const cfg = "wp9/graft-wp9-git.jsonc";
  writeFileSync(`${ROOT}${cfg}`, JSON.stringify({
    name: "graft-wp9-git", main: "../src/wp9/git.ts", compatibility_date: "2026-10-01", workers_dev: true, preview_urls: false,
    containers: [{ class_name: "Shard9", scheduling_policy: "durable_object", images: { git: { dockerfile: "./git/Dockerfile" } } }],
    durable_objects: { bindings: [{ name: "SHARD", class_name: "Shard9" }, { name: "NET", class_name: "Net9" }, { name: "POOL", class_name: "Pool9" },
      { name: "DRIVE", class_name: "Drive9" }] },
    r2_buckets: [{ binding: "GIT", bucket_name: "graft-wp9-git" }],
    migrations: [{ tag: "v1", new_sqlite_classes: ["Shard9", "Net9", "Pool9", "Drive9"] }],
    limits: { cpu_ms: 300_000 },
    observability: { enabled: true },
  }, null, 2));
  wrangler(["deploy", "-c", cfg]);
  wrangler(["secret", "put", "M0_TOKEN", "-c", cfg], TOKEN);
}

const lines = (f: string) => readFileSync(f, "utf8").trim().split("\n");
const pct = (xs: number[], p: number) => { if (!xs.length) return null; const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };

async function run(): Promise<void> {
  const run = opt.run!, bed = `b34-${run}`, seconds = Number(opt.seconds), clients = Number(opt.clients), mode = opt.mode!;
  const args = {
    linux: { commits: lines(`${DATA}/linux.commits`), merges: lines(`${DATA}/linux.merges`).map((l) => l.trim()) },
    git: { commits: lines(`${DATA}/git.commits`), merges: lines(`${DATA}/git.merges`).map((l) => l.trim()) },
  };
  log(`bed ${bed}: init linux on a, b`);
  // The load starts about 20 s after both warm sites are ready; the tip clock is set when the load starts (`/tip`).
  const hyst = { alike: opt.alike, upMs: Number(opt.up) * 1000, downMs: Number(opt.down) * 1000 };
  await call("/init", { bed, cfg: { net: "linux", src: "linux", owner: "mono", budget: 6, warm: ["a", "b"], spare: ["c"], ...hyst } });
  // Wait for both warm sites' base shards (8 GB each from the bucket).
  for (let i = 0; ; i++) {
    const [v] = await call<{ sites: Record<string, { ready: string[] }>; timeline: object[] }[]>("/view", { bed, nets: ["linux"] });
    const ready = Object.values(v.sites ?? {}).filter((s) => s.ready.length).length;
    if (ready >= 2) { log("both warm sites ready", JSON.stringify(v.timeline.filter((e) => (e as { ev?: string }).ev))); break; }
    if (i > 80) throw new Error(`warm sites not ready: ${JSON.stringify(v).slice(0, 600)}`);
    await sleep(10_000);
  }
  // Journey 9's git work on the monorepo (scripts/cost_agents.py, shard_vcpu): 4.17 agent pushes/s with a head-push
  // test merge, 1.25 agent CI clone-cache misses/s, 0.25 agent clone misses/s, 10 queue merges a minute, plus human
  // pushes and clone-cache hits for time to first byte.
  const streams: Stream[] = [
    { net: "linux", kind: "push", cls: "Agent", rate: 4.17, from: 0, to: seconds },
    { net: "linux", kind: "miss", cls: "Agent", rate: 1.25, from: 0, to: seconds, mode },
    { net: "linux", kind: "miss", cls: "Agent", rate: 0.25, from: 0, to: seconds, mode, near: true },
    { net: "linux", kind: "merge", cls: "MergeQueue", rate: 10 / 60, from: 0, to: seconds },
    { net: "linux", kind: "human", cls: "Human", rate: 0.3, from: 0, to: seconds },
    { net: "linux", kind: "hit", cls: "Human", rate: 2, from: 0, to: seconds },
    // 600 s: a second network turns hot.
    { net: "gitnet", kind: "push", cls: "Agent", rate: 2, from: 600, to: seconds },
    { net: "gitnet", kind: "miss", cls: "Agent", rate: 3, from: 600, to: seconds, mode },
    { net: "gitnet", kind: "human", cls: "Human", rate: 0.3, from: 600, to: seconds },
    // 840 s: the hostile owner's three networks.
    ...[1, 2, 3].flatMap((h) => [
      { net: `hostile${h}`, kind: "miss", cls: "Agent", rate: 3, from: 840, to: seconds, mode } as Stream,
      { net: `hostile${h}`, kind: "push", cls: "Agent", rate: 1, from: 840, to: seconds } as Stream,
    ]),
  ];
  if (opt["j9-only"]) {
    const f = Number(opt.scale);
    streams.splice(0, streams.length, ...streams.filter((st) => st.net === "linux").map((st) => ({ ...st, rate: st.kind === "hit" ? st.rate : st.rate * f })));
  }
  const start = Date.now() + 20_000;
  const spec: Omit<DriveSpec, "index" | "site"> = { bed, run: `${bed}-load`, clients, start, seconds, home: { linux: "a", gitnet: "c", hostile1: "c", hostile2: "c", hostile3: "c" }, streams, args,
    tipPerS: Number(opt["tip-per-s"]) };
  writeFileSync(`${ROOT}results/raw/wp9-b0-34-${run}.spec.json`, JSON.stringify({ run, bed, mode, start, seconds, clients, hyst, tipPerS: spec.tipPerS }));
  await call("/tip", { bed, net: "linux", tipStart: start, tipPerS: spec.tipPerS });
  log(await call("/start", { spec, sites: ["a", "b", "c"] }));
  const at = (s: number) => sleep(Math.max(0, start + s * 1000 - Date.now()));
  const loads: object[] = [];
  const snap = async (label: string) => { try { loads.push({ at: Date.now() - start, label, ...(await call<object>("/load", { bed }, 60_000)) }); } catch (e) { log("load snap failed", String(e).slice(0, 100)); } };
  const snapsPath = `${ROOT}results/raw/wp9-b0-34-${run}.snaps.jsonl`;
  let stopSnaps = false;
  const snapper = (async () => { for (let t = 30; !stopSnaps && t < seconds; t += 30) { await at(t); if (stopSnaps) break; await snap(`t${t}`); appendFileSync(snapsPath, JSON.stringify(loads[loads.length - 1] ?? {}) + "\n"); } })();
  if (opt["j9-only"]) {
    await at(seconds - 10); stopSnaps = true; await snapper;
  } else {
  await at(400);
  await at(420); log("kill site a", JSON.stringify(await call("/kill", { bed, site: "a", nets: ["linux"] })));
  await at(590); await snap("before second network");
  log("init gitnet"); await call("/init", { bed, cfg: { net: "gitnet", src: "git", owner: "other", budget: 2, warm: ["c", "b"], spare: [], ...hyst } });
  await at(830); await snap("before hostile");
  for (const h of [1, 2, 3]) await call("/init", { bed, cfg: { net: `hostile${h}`, src: "git", owner: "hostile", budget: 2, warm: ["c", "b"], spare: [], ...hyst } });
  log("hostile owner's networks up");
  await at(seconds - 10); stopSnaps = true; await snapper;
  }
  await at(seconds + 200);
  let results: ({ colo: string; rows: number[][]; errs: Record<string, number> } | null)[] = [];
  for (let i = 0; i < 30; i++) {
    results = await call("/result", { run: spec.run, clients }, 120_000);
    if (results.every(Boolean)) break;
    log(`waiting for ${results.filter((r) => !r).length} clients`); await sleep(20_000);
  }
  const nets = await call<object[]>("/view", { bed, nets: ["linux", "gitnet", "hostile1", "hostile2", "hostile3"] });
  const raw = { run, bed, mode, start, seconds, clients, streams, results, nets, loads };
  writeFileSync(`${ROOT}results/raw/wp9-b0-34-${run}.json`, JSON.stringify(raw));
  const summary = summarize(raw);
  writeFileSync(`${ROOT}results/raw/wp9-b0-34-${run}.summary.json`, JSON.stringify(summary, null, 1));
  console.log(JSON.stringify(summary, null, 1));
  log("stopping containers"); await call("/stop", { bed, nets: ["linux", "gitnet", "hostile1", "hostile2", "hostile3"] }, 300_000);
}

type Raw = { streams: Stream[]; results: ({ colo: string; rows: number[][]; errs: Record<string, number> } | null)[]; nets: any[]; seconds: number };
function summarize(raw: Raw): object {
  const rows = raw.results.flatMap((r) => r?.rows ?? []);
  const phases: [string, number, number][] = [["journey 9", 0, 420], ["site a killed", 420, 600], ["second network", 600, 840], ["hostile owner", 840, raw.seconds]];
  const byStream = raw.streams.map((st, si) => {
    const out: Record<string, unknown> = { stream: `${st.net} ${st.kind} ${st.cls}${st.near ? " near" : ""}` };
    for (const [name, a, b] of phases) {
      const rs = rows.filter((r) => r[1] === si && r[0] >= a * 1000 && r[0] < b * 1000);
      if (!rs.length) continue;
      const ok = rs.filter((r) => r[2] === 0);
      out[name] = { n: rs.length, ok: ok.length, shed: rs.filter((r) => r[2] === 1).length, err: rs.filter((r) => r[2] === 2).length, noShard: rs.filter((r) => r[2] === 3).length,
        msP50: pct(ok.map((r) => r[3]), 0.5), msP99: pct(ok.map((r) => r[3]), 0.99), ttfbP99: pct(ok.map((r) => r[4]).filter((x) => x >= 0), 0.99),
        queueP99: pct(ok.map((r) => r[5]).filter((x) => x >= 0), 0.99), cpuMean: ok.length ? Math.round((ok.reduce((s, r) => s + r[6], 0) / ok.length) * 1000) / 1000 : null };
    }
    return out;
  });
  // Clone time to first byte: hits and misses together, per phase (misses that were shed or failed count at their ms).
  const clones = raw.streams.map((st, si) => (st.kind === "hit" || st.kind === "miss" ? si : -1)).filter((x) => x >= 0);
  const ttfb = Object.fromEntries(phases.map(([name, a, b]) => {
    const rs = rows.filter((r) => clones.includes(r[1]) && r[0] >= a * 1000 && r[0] < b * 1000);
    const t = rs.map((r) => (r[2] === 0 && r[4] >= 0 ? r[4] : r[3]));
    return [name, { n: t.length, p50: pct(t, 0.5), p99: pct(t, 0.99), linuxHitP99: pct(rs.filter((r) => raw.streams[r[1]].kind === "hit").map((r) => r[4]), 0.99) }];
  }));
  const k = raw.nets.map((n) => ({ net: n.net, owner: n.cfg?.owner,
    maxK: Object.fromEntries(Object.keys(n.sites ?? {}).map((s) => [s, Math.max(1, ...(n.timeline ?? []).filter((e: any) => e.site === s && e.k).map((e: any) => e.k))])),
    denials: n.denials, apiCalls: Math.max(0, ...(n.timeline ?? []).filter((e: any) => e.apiCalls !== undefined).map((e: any) => e.apiCalls)) }));
  const errs: Record<string, number> = {};
  for (const r of raw.results) for (const [e, c] of Object.entries(r?.errs ?? {})) errs[e] = (errs[e] ?? 0) + c;
  // Third pass: k, own CPU, and judged CPU per site every 30 s, seconds from the start of load.
  const start = (raw as unknown as { start: number }).start;
  const kTimeline = raw.nets.map((n) => ({ net: n.net, sites: Object.fromEntries(Object.keys(n.sites ?? {}).map((s) => [s,
    (n.timeline ?? []).filter((e: any) => e.site === s && e.k !== undefined).filter((_: unknown, i: number) => i % 3 === 0)
      .map((e: any) => [Math.round((e.at - start) / 1000), e.k, e.cpu, e.judged ?? e.cpu, e.ready])])) }));
  // Miss TTFB alone, per phase, and the clone-cache hit TTFB alone.
  const missTtfb = Object.fromEntries(phases.map(([name, a, b]) => {
    const rs = rows.filter((r) => raw.streams[r[1]].kind === "miss" && r[0] >= a * 1000 && r[0] < b * 1000);
    const t = rs.map((r) => (r[2] === 0 && r[4] >= 0 ? r[4] : r[3]));
    return [name, { n: t.length, p50: pct(t, 0.5), p99: pct(t, 0.99), notOk: rs.filter((r) => r[2] !== 0).length }];
  }));
  const events = raw.nets.map((n) => ({ net: n.net, ev: (n.timeline ?? []).filter((e: any) => e.ev).map((e: any) => ({ t: Math.round((e.at - start) / 1000), ev: e.ev, site: e.site, name: e.name, ms: e.ms, copyMBps: e.r?.copyMBps })) }));
  return { byStream, ttfb, missTtfb, k, kTimeline, events, errs, colos: raw.results.map((r) => r?.colo) };
}

const cmd = positionals[0];
if (cmd === "deploy") deploy();
else if (cmd === "run") await run();
else if (cmd === "summarize") { const raw = JSON.parse(readFileSync(`${ROOT}results/raw/wp9-b0-34-${opt.run}.json`, "utf8")); console.log(JSON.stringify(summarize(raw), null, 1)); }
else if (cmd === "stop") { const bed = `b34-${opt.run}`; console.log(await call("/stop", { bed, nets: ["linux", "gitnet", "hostile1", "hostile2", "hostile3"] }, 300_000)); }
else if (cmd === "delete") { wrangler(["delete", "--name", "graft-wp9-git", "--force"]); }
else console.log("usage: node run/wp9-git.ts deploy | run [--run X] | summarize --run X | stop --run X | delete");
