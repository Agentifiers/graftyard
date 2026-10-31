// M0-WP9 eighth pass: finishes a run whose runner died after `/start` (the clients run on their own alarms). It does
// what `run/wp9.ts run` does after `/start`: keeps the newest good member snapshot every 20 s until the load ends plus
// 3 s, saves it with the member logs and status as `.members.json`, waits, and then collects.
//   node run/wp9-rescue.ts <row> --run X
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: false, options: { run: { type: "string" }, wait: { type: "string" } } });
const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = "https://graft-wp9-load.<workers-subdomain>.workers.dev";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function call<T>(path: string, body: unknown, timeoutMs = 120_000): Promise<T> {
  const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
  const text = await res.text();
  if (!res.ok) throw new Error(`${path}: ${res.status} ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

const [row] = positionals, run = opt.run as string;
const specPath = `${ROOT}results/raw/wp9-${row}-${run}.spec.json`;
if (!existsSync(specPath)) throw new Error("no spec for that run");
const spec = (JSON.parse(readFileSync(specPath, "utf8")) as { spec: { group: string; learners: number[]; start: number; seconds: number } }).spec;
const { group, learners } = spec;
const keep: { voters: any[]; learners: any[]; takenAt: Record<string, number> } = { voters: [], learners: [], takenAt: {} };
const take = async () => {
  const r = await call<{ voters: any[]; learners: any[] }>("/w9stats", { group, learners, reset: false }, 30_000).catch(() => null);
  if (!r) return;
  r.voters.forEach((v, i) => { if (!v.error && (v.reqsSeconds ?? 0) >= (keep.voters[i]?.reqsSeconds ?? 0) * 0.5) { keep.voters[i] = v; keep.takenAt[v.site] = Date.now(); } else keep.voters[i] ??= v; });
  r.learners.forEach((v, i) => { if (!v.error) keep.learners[i] = v; else keep.learners[i] ??= v; });
};
while (Date.now() < spec.start + spec.seconds * 1000 + 3_000) { await take(); await sleep(20_000); }
await take();
const logs = await call("/log", { group }, 60_000).catch((e) => ({ error: String(e) }));
const status = await call("/status", { group });
writeFileSync(`${ROOT}results/raw/wp9-${row}-${run}.members.json`, JSON.stringify({ at: Date.now(), snap: keep, status, logs }));
await sleep(Number(opt.wait ?? 75) * 1000);
execFileSync("node", [`${ROOT}run/wp9.ts`, "collect", row, "--run", run], { stdio: "inherit" });
