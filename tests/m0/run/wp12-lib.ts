// M0-WP12 runner library: deploy, group setup, load start and collection, and small helpers shared by run/wp12.ts.
// Scripts are graft-wp12-a/b/c (members) and graft-wp12-load (load clients, edge routes, and the wp12 fault plan), and
// the bucket graft-wp12-stage. Nothing here names another bed's script.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import type { Spec } from "../src/wp12/load.ts";
import type { XCmd, XEv, XOut } from "../src/wp12/lease.ts";

export const ROOT = new URL("../", import.meta.url).pathname;
export const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
export const PREFIX = "graft-wp12";
export const URL_ = `https://${PREFIX}-load.<workers-subdomain>.workers.dev`;
export const BUCKET = "graft-wp12-stage";
const COMPAT = "2026-10-01";
export const SITES = ["a", "b", "c"] as const;
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export const stamp = () => new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13);
export const RAW = `${ROOT}results/raw`;
mkdirSync(RAW, { recursive: true });

export function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });
}
export async function call<T>(path: string, body: unknown = {}, timeoutMs = 120_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
      const text = await res.text();
      if (!res.ok) throw new Error(`${path}: ${res.status} ${text.slice(0, 300)}`);
      return JSON.parse(text) as T;
    } catch (e) {
      // Eighth pass: a runner-side timeout is retried too (r15 placed and r23 no-learners lost a phase to one).
      if (attempt >= 2 || !/fetch failed|ECONNRESET|UND_ERR|502|503|1101|TimeoutError/.test(String(e))) throw e;
      await sleep(2_000);
    }
  }
}

export function deploy(): void {
  mkdirSync(`${ROOT}wp12`, { recursive: true });
  const member = (k: string, phase: "own" | "cycle") => {
    const path = `wp12/${PREFIX}-${k}-${phase}.jsonc`;
    writeFileSync(`${ROOT}${path}`, JSON.stringify({
      name: `${PREFIX}-${k}`, main: "../src/wp12/member.ts", compatibility_date: COMPAT, workers_dev: false, preview_urls: false,
      vars: { SITE: k }, limits: { cpu_ms: 300_000 },
      durable_objects: { bindings: [...(phase === "own" ? [k] : SITES).map((s) => ({ name: `MEMBER_${s.toUpperCase()}`, class_name: "RaftMember",
        ...(s === k ? {} : { script_name: `${PREFIX}-${s}` }) })),
        ...(phase === "cycle" ? [{ name: "FAULTS", class_name: "FaultControl", script_name: `${PREFIX}-load` }] : [])] },
      migrations: [{ tag: "v1", new_sqlite_classes: ["RaftMember"] }],
      observability: { enabled: true },
    }, null, 2));
    return path;
  };
  try { wrangler(["r2", "bucket", "create", BUCKET, "--location", "enam"]); } catch { /* exists */ }
  const load = (phase: "own" | "cycle") => {
    const path = `wp12/${PREFIX}-load-${phase}.jsonc`;
    writeFileSync(`${ROOT}${path}`, JSON.stringify({
      name: `${PREFIX}-load`, main: "../src/wp12/load.ts", compatibility_date: COMPAT, workers_dev: true, preview_urls: false,
      limits: { cpu_ms: 300_000 },
      durable_objects: { bindings: [...(phase === "cycle" ? SITES.map((s) => ({ name: `MEMBER_${s.toUpperCase()}`, class_name: "RaftMember", script_name: `${PREFIX}-${s}` })) : []),
        { name: "LOAD", class_name: "Load9" }, { name: "FAULTS", class_name: "FaultControl" }] },
      r2_buckets: [{ binding: "STAGE", bucket_name: BUCKET }, { binding: "STAGE_C", bucket_name: BUCKET }],
      migrations: [{ tag: "v1", new_sqlite_classes: ["Load9", "FaultControl"] }],
      observability: { enabled: true },
    }, null, 2));
    return path;
  };
  // The load script owns FaultControl, which the members bind, so it deploys first without the member bindings.
  const first = !existsSync(`${ROOT}wp12/.deployed`);
  if (first) { wrangler(["deploy", "-c", load("own")]); for (const k of SITES) wrangler(["deploy", "-c", member(k, "own")]); }
  for (const k of SITES) wrangler(["deploy", "-c", member(k, "cycle")]);
  wrangler(["deploy", "-c", load("cycle")]);
  if (first) wrangler(["secret", "put", "M0_TOKEN", "-c", load("cycle")], TOKEN);
  writeFileSync(`${ROOT}wp12/.deployed`, new Date().toISOString());
}

export type Status = { site: string; role: string; term: number; colo?: string; leader?: string; error?: string; boots?: number[]; stalledSince?: number | null };

/** Creates the group, moves its leader to site A (enam), sets learners, and commits the seed chunks. */
export async function setup(group: string, learners: number[], seedChunks: XEv[][] = [], home = "a"): Promise<Status[]> {
  await call("/op", { group, op: { kind: "put", key: "warm", value: "1" } });
  for (let i = 0; i < 20; i++) {
    const st = await call<Status[]>("/status", { group });
    const leader = st.find((s) => s.role === "leader")?.site;
    if (leader === home) break;
    if (leader) await call("/transfer", { group, to: home }).catch(() => {});
    else await call("/op", { group, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {});
    await sleep(1_000);
  }
  await call("/learners", { group, slots: learners });
  for (const events of seedChunks) {
    const r = await call<{ out: XOut | null }>("/x", { group, cmd: { t: "Seed", events } satisfies XCmd, key: "seed" });
    if (!r.out?.ok) throw new Error(`seed failed: ${JSON.stringify(r).slice(0, 200)}`);
  }
  for (let i = 0; i < 3; i++) await call("/op", { group, op: { kind: "put", key: "warm", value: String(i) } });
  return call<Status[]>("/status", { group });
}

export const HINTS: DurableObjectLocationHint[] = ["enam", "enam", "wnam", "weur", "apac"];
/** Journey 9 / A1 at 500 agents, as M0-WP9's B0-29 replays it (`scripts/cost_agents.py --a1`). */
export function a1(agents = 500): Omit<Spec, "run" | "group" | "index" | "clients" | "start" | "seconds"> {
  const f = agents / 500;
  return { agents, pushEvery: 120, pushers: [], packMin: 10_000, packMax: 70_000, bigShare: 0.05, bigBytes: 1 << 20,
    agentSide: 21.0 * f, human: 1.25, bot: 1.0, refReads: 33, apiReads: 200, agentReads: 43, mergesPerMin: 10, pollers: 500, learners: [] };
}
export function spec(base: { run: string; group: string; seconds: number; clients: number; startIn?: number }, rest: Omit<Spec, "run" | "group" | "index" | "clients" | "start" | "seconds">): Spec {
  const pushers = Array.from({ length: base.clients }, (_, i) => i).filter((i) => HINTS[i % HINTS.length] === "enam");
  return { ...rest, run: base.run, group: base.group, index: 0, clients: base.clients, start: Date.now() + (base.startIn ?? 30_000), seconds: base.seconds, pushers };
}
export async function start(s: Spec): Promise<void> { await call("/start", { spec: s, hints: HINTS }); }

/** Member counters, read every 20 s and the newest good reading of each kept (members lose them on a reset). */
export async function watch(group: string, learners: number[], until: number, tick?: (t: number) => Promise<void>): Promise<{ voters: any[]; learners: any[] }> {
  const keep = { voters: [] as any[], learners: [] as any[] };
  const take = async () => {
    const r = await call<{ voters: any[]; learners: any[] }>("/w9stats", { group, learners, reset: false }, 40_000).catch(() => null);
    if (!r) return;
    r.voters.forEach((v, i) => { if (!v.error && (v.reqsSeconds ?? 0) >= (keep.voters[i]?.reqsSeconds ?? 0) * 0.5) keep.voters[i] = v; else keep.voters[i] ??= v; });
    r.learners.forEach((v, i) => { if (!v.error) keep.learners[i] = v; else keep.learners[i] ??= v; });
  };
  while (Date.now() < until) { await sleep(Math.min(20_000, Math.max(1_000, until - Date.now()))); if (tick) await tick(Date.now()); await take(); }
  await take();
  return keep;
}
export async function results(run: string, clients: number): Promise<any[]> {
  const out: any[] = [];
  for (let i = 0; i < clients; i++) {
    for (let a = 0; a < 20; a++) {
      const r = await call<any>("/result", { run, i }).catch((e) => ({ error: String(e) }));
      if (!r.pending) { out.push(r); break; }
      if (a === 19) { out.push(r.partial ?? r); break; }
      await sleep(5_000);
    }
  }
  return out;
}

export const pct = (xs: number[], p: number) => { if (!xs.length) return null; const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
export const dist = (xs: number[]) => ({ n: xs.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99), max: pct(xs, 1) });
export function save(name: string, v: unknown): string {
  const path = `${RAW}/wp12-${name}.json`;
  writeFileSync(path, JSON.stringify(v, null, 1));
  return path;
}
export const log = (...m: unknown[]) => console.log(new Date().toISOString().slice(11, 19), ...m);
/** Commit latency for every voter's admitted commands, all classes. */
export function commitDist(voters: any[]): ReturnType<typeof dist> {
  const all: number[] = [];
  for (const v of voters) if (v?.lat) for (const c of ["Human", "MergeQueue", "Bot", "Agent"]) all.push(...v.lat[c]);
  return dist(all);
}
