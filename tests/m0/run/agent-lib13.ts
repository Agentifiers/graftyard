// M0-WP13 driver helpers shared by run/agent-l0*.ts: a run's raw-row file, cells with counts, and the bed.

import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { ROOT, api, harness, tok } from "./agent-lib.ts";

export const RAW = `${ROOT}results/raw`;
export const HOOKS = `https://graft-wp13-hooks.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
export const V1 = "/api/graftyard/v1/repos/acme/mono";
export const V3 = "/api/v3/repos/acme/mono";

export class Rows {
  path: string;
  cells = new Map<string, { n: number; refused: number; statuses: Record<string, number>; sample: string; expect: string }>();
  constructor(row: string, run: string) {
    mkdirSync(RAW, { recursive: true });
    this.path = `${RAW}/wp13-${row}-${run}.jsonl`;
    writeFileSync(this.path, "");
  }
  add(cell: string, expect: "refused" | "accepted" | "info", status: number | string, refused: boolean, extra: Record<string, unknown> = {}): void {
    appendFileSync(this.path, JSON.stringify({ cell, expect, status, refused, ...extra }) + "\n");
    const c = this.cells.get(cell) ?? { n: 0, refused: 0, statuses: {}, sample: "", expect };
    c.n++; if (refused) c.refused++;
    c.statuses[String(status)] = (c.statuses[String(status)] ?? 0) + 1;
    if (!c.sample && (refused || expect !== "refused")) c.sample = String(extra.message ?? "").slice(0, 160);
    this.cells.set(cell, c);
  }
  /** Cells off their expectation. */
  bad(): string[] {
    return [...this.cells].filter(([, c]) => c.expect === "refused" ? c.refused !== c.n : c.expect === "accepted" ? c.refused !== 0 : false)
      .map(([k, c]) => `${k}: ${c.refused}/${c.n} refused ${JSON.stringify(c.statuses)}`);
  }
  print(): void {
    for (const [k, c] of this.cells) console.log(`${k.padEnd(58)} ${String(c.refused).padStart(4)}/${String(c.n).padEnd(4)} refused  ${JSON.stringify(c.statuses)}  ${c.sample}`);
  }
  summary(): object { return Object.fromEntries(this.cells); }
}

/** Keeps one human token under DEMOTE_PER_MIN (60 committed writes a minute): at most `per` calls in any 60 s. */
export class Pace {
  private at: number[] = [];
  private per: number;
  constructor(per = 55) { this.per = per; }
  async next(): Promise<void> {
    for (;;) {
      const now = Date.now();
      this.at = this.at.filter((t) => now - t < 61_000);
      if (this.at.length < this.per) { this.at.push(now); return; }
      await new Promise((r) => setTimeout(r, 61_000 - (now - this.at[0]) + 50));
    }
  }
}

export interface Attempt { refused: boolean; status: string; text: string; ms: number }
/** Runs `n` attempts of one cell, joins the host's audit rows (the rule that refused), and records them. */
export async function cell(rows: Rows, U: string, run: string, name: string, expect: "refused" | "accepted", n: number, k: number,
                           f: (i: number, probe: string) => Promise<Attempt>): Promise<void> {
  const { pool } = await import("./agent-lib.ts");
  const out = await pool(n, k, async (i) => {
    const probe = `${run}-${name}-${i}`;
    try { return { probe, ...(await f(i, probe)) }; } catch (e) { return { probe, refused: false, status: "driver-error", text: String(e).slice(0, 200), ms: 0 }; }
  });
  const audit = await harness(U, "/m0/audit", out.map((r) => r.probe)) as ({ rule?: string; ok: boolean } | null)[];
  out.forEach((r, j) => rows.add(name, expect, r.status, r.refused, { message: r.text, rule: audit[j] ? (audit[j]!.ok ? "ok" : audit[j]!.rule) : null, ms: r.ms }));
  const rules = [...new Set(audit.map((a) => a ? (a.ok ? "ok" : a.rule) : "-"))].join(",");
  console.log(`${name.padEnd(64)} ${String(out.filter((r) => r.refused).length).padStart(3)}/${n}  rules=${rules}  ${out.find((r) => r.refused)?.text.slice(0, 90) ?? out[0]?.text.slice(0, 90)}`);
}

export const msg = (b: any): string => typeof b === "object" && b ? String(b.message ?? JSON.stringify(b)).slice(0, 200) : String(b).slice(0, 200);

/** Turns `agents_enabled` (and optionally `code_mode`) on or off as the org admin. */
export async function orgSet(U: string, body: { agents_enabled?: boolean; code_mode?: boolean }): Promise<void> {
  const r = await api(U, "PATCH", "/api/graftyard/v1/orgs/acme/agents", await tok("alice"), body);
  if (r.status !== 200) throw new Error(`org patch: ${r.status} ${JSON.stringify(r.body)}`);
}

export async function hookLog(since: number): Promise<{ app: string; event: string; id: string; at: number; sig: number; body: string }[]> {
  const { TOKEN } = await import("./agent-lib.ts");
  const r = await fetch(`${HOOKS}/log?since=${since}`, { headers: { authorization: `Bearer ${TOKEN}` } });
  return r.json() as any;
}
export async function hookReset(): Promise<void> {
  const { TOKEN } = await import("./agent-lib.ts");
  await fetch(`${HOOKS}/reset`, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` } });
}

export { api, harness, tok };
