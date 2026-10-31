// B0-30 runner (build/BUILD-PLAN.md, M0-WP9): 30 days of hot-group storage, accelerated, on `graft-wp9-store`
// (src/wp9/store.ts).
//
//   node run/wp9-store.ts deploy
//   node run/wp9-store.ts run [--run X] [--meta-gb 4] [--days 30]
//   node run/wp9-store.ts purge
//   node run/wp9-store.ts delete
//
// Steps: seed the metadata tier, then replay journey 9 in chunks of simulated time, archiving above LOG_MAX_BYTES after
// each chunk, then read archived ranges back. Every response is appended to results/raw/wp9-b0-30-<run>.jsonl as it
// arrives, so a stalled run keeps what it measured.

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string", default: "r1" }, "meta-gb": { type: "string", default: "4" }, days: { type: "string", default: "30" },
  chunk: { type: "string", default: "3600" } } });
const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = "https://graft-wp9-store.<workers-subdomain>.workers.dev";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });
}
async function call<T>(body: object, timeoutMs = 300_000): Promise<T> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(URL_, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 400)}`);
      return JSON.parse(t) as T;
    } catch (e) {
      if (i >= 4 || /SQLITE_FULL|disk is full/i.test(String(e))) throw e;
      await sleep(5_000 * (i + 1));
    }
  }
}

function deploy(): void {
  for (const [b, loc] of [["graft-wp9-arch-a", "enam"], ["graft-wp9-arch-b", "weur"]]) {
    try { wrangler(["r2", "bucket", "create", b, "--location", loc]); } catch { /* exists */ }
  }
  const cfg = "wp9/graft-wp9-store.jsonc";
  writeFileSync(`${ROOT}${cfg}`, JSON.stringify({
    name: "graft-wp9-store", main: "../src/wp9/store.ts", compatibility_date: "2026-10-01", workers_dev: true, preview_urls: false,
    limits: { cpu_ms: 300_000 },
    durable_objects: { bindings: [{ name: "STORE", class_name: "Store9" }] },
    r2_buckets: [{ binding: "ARCH_A", bucket_name: "graft-wp9-arch-a" }, { binding: "ARCH_B", bucket_name: "graft-wp9-arch-b" }],
    migrations: [{ tag: "v1", new_sqlite_classes: ["Store9"] }],
    observability: { enabled: true },
  }, null, 2));
  wrangler(["deploy", "-c", cfg]);
  wrangler(["secret", "put", "M0_TOKEN", "-c", cfg], TOKEN);
}

async function run(): Promise<void> {
  const run = opt.run!, name = `b0-30-${run}`, out = `${ROOT}results/raw/wp9-b0-30-${run}.jsonl`;
  const rec = (step: string, r: object) => { appendFileSync(out, JSON.stringify({ at: new Date().toISOString(), step, ...r }) + "\n"); console.log(step, JSON.stringify(r).slice(0, 300)); };
  const t0 = Date.now();
  // Seed: 2.2 million rows of about 1.8 KB is about 4 GB of row bytes.
  const metaBytes = Number(opt["meta-gb"]) * 1e9, rowBytes = 1800, rows = Math.ceil(metaBytes / rowBytes), batch = 20_000;
  const done0 = existsDone(out, "seed");
  for (let from = done0; from < rows; from += batch) {
    const r = await call<{ size: number; sqliteFull: number; ms: number }>({ name, op: "seed", from, rows: Math.min(batch, rows - from), rowBytes });
    rec("seed", { from, ...r });
    if (r.sqliteFull) break;
  }
  rec("seeded", { ...(await call<object>({ name, op: "stats" })), wallS: Math.round((Date.now() - t0) / 1000) });
  // Replay: chunks of simulated seconds, archive after each.
  const total = Number(opt.days) * 86_400, chunk = Number(opt.chunk);
  const t1 = Date.now();
  for (;;) {
    const s = await call<{ sim: number; size: number; full: boolean; sqliteFull: number; ms: number }>({ name, op: "replay", seconds: 0 });
    if (s.sim >= total) break;
    const r = await call<{ sim: number; size: number; full: boolean; sqliteFull: number; ms: number }>({ name, op: "replay", seconds: Math.min(chunk, total - s.sim) });
    let a: { over: boolean; ranges: number } = { over: true, ranges: 0 };
    let arch = 0;
    while (a.over) { a = await call({ name, op: "archive", max: 8 }); arch += a.ranges; if (!a.ranges) break; }
    rec("replay", { ...r, archRanges: arch, archive: a, simDays: Math.round((r.sim / 86_400) * 100) / 100 });
    if (r.full) { rec("sqlite full", r); break; }
  }
  rec("replayed", { ...(await call<object>({ name, op: "stats" })), wallS: Math.round((Date.now() - t1) / 1000) });
  rec("readback", await call<object>({ name, op: "readback", sample: 30 }, 600_000));
}

/** Resume support: the next seed row after the last recorded seed step. */
function existsDone(path: string, step: string): number {
  try {
    const ls = readFileSync(path, "utf8").trim().split("\n").map((l) => JSON.parse(l) as { step: string; from: number; done: number });
    const s = ls.filter((l) => l.step === step);
    return s.length ? s[s.length - 1].from + s[s.length - 1].done : 0;
  } catch { return 0; }
}

const cmd = positionals[0];
if (cmd === "deploy") deploy();
else if (cmd === "run") await run();
else if (cmd === "purge") { for (;;) { const r = await call<{ deleted: number }>({ name: "-", op: "purge" }); console.log(r); if (!r.deleted) break; } }
else if (cmd === "delete") {
  wrangler(["delete", "--name", "graft-wp9-store", "--force"]);
  for (const b of ["graft-wp9-arch-a", "graft-wp9-arch-b"]) wrangler(["r2", "bucket", "delete", b]);
}
else console.log("usage: node run/wp9-store.ts deploy | run [--run X] | purge | delete");
