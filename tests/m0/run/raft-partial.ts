// B0-25 on the M0-WP2 Raft prototype: partial Durable Object failure. 22% of member calls (edge to member and member to
// member) fail, and 22% of member storage writes fail and reset the object. A "push" here is one write operation, and
// the client retries a failed one up to three attempts with a new request id each time, as a git client re-pushes.
// The design's arithmetic for a real push (token, `Authz`, commit: three member calls) is checked by
// running each push as three sequential writes on three groups, all of which must succeed in one attempt.
//
//   node run/raft-partial.ts [--pushes 300] [--concurrency 10] [--rate 0.22]

import { mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { raft } from "./raft.ts";

const { values: opt } = parseArgs({ options: { pushes: { type: "string", default: "300" }, concurrency: { type: "string", default: "10" },
  rate: { type: "string", default: "0.22" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const rate = Number(opt.rate);
const groups = ["token", "authz", "repo"].map((g) => `b0-25-${g}-${opt.run}`);

for (const g of groups) await raft("/op", { group: g, op: { kind: "put", key: "warm", value: "1" } });
const from = Date.now() + 3_000;
await raft("/faults", { plan: { from, rules: [{ target: "call", mode: "error", rate }, { target: "write", rate }] } });
await new Promise((r) => setTimeout(r, 3_500));

/** One push attempt: three member-call steps on three groups, each one client operation through the edge, in order. */
async function attempt(n: number, a: number): Promise<{ ok: boolean; ms: number; error?: string }> {
  const t = Date.now();
  try {
    await raft("/op", { group: groups[0], strict: true, op: { kind: "get", key: "tok" } });
    await raft("/op", { group: groups[1], strict: true, op: { kind: "get", key: "authz" } });
    await raft("/op", { group: groups[2], strict: true, op: { kind: "put", key: `ref${n % 32}`, value: `${n}.${a}` } });
    return { ok: true, ms: Date.now() - t };
  } catch (e) {
    return { ok: false, ms: Date.now() - t, error: String(e).slice(0, 600) };
  }
}

const rows: { push: number; attempts: number; ok: boolean; ms: number[]; errors: string[] }[] = [];
let next = 0;
await Promise.all(Array.from({ length: Number(opt.concurrency) }, async () => {
  while (next < Number(opt.pushes)) {
    const n = next++;
    const row = { push: n, attempts: 0, ok: false, ms: [] as number[], errors: [] as string[] };
    while (row.attempts < 3 && !row.ok) {
      const r = await attempt(n, row.attempts++);
      row.ms.push(r.ms); row.ok = r.ok; if (r.error) row.errors.push(r.error);
    }
    rows.push(row);
  }
}));
await raft("/faults", { plan: { rules: [] } });

const share = (k: number) => rows.filter((r) => r.ok && r.attempts <= k).length / rows.length;
const firstMs = rows.filter((r) => r.ok && r.attempts === 1).map((r) => r.ms[0]).sort((a, b) => a - b);
const summary = {
  row: "B0-25", run: opt.run, at: new Date().toISOString(), rate, pushes: rows.length,
  succeededWithin: { 1: share(1), 2: share(2), 3: share(3) },
  firstAttemptMs: { p50: firstMs[Math.floor(firstMs.length / 2)], p99: firstMs[Math.floor(firstMs.length * 0.99)] },
  errors: Object.entries(rows.flatMap((r) => r.errors).reduce((m, e) => { m[e] = (m[e] ?? 0) + 1; return m; }, {} as Record<string, number>)),
  verdict: share(3) >= 0.96 ? "Pass" : share(3) < 0.84 ? "Fail" : "Between pass and fail",
};
mkdirSync(`${ROOT}results`, { recursive: true });
writeFileSync(`${ROOT}results/b0-25.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify(summary, null, 2));
