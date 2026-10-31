// M0-WP12 runner: leases, the activity feed, and the core tier (build/BUILD-PLAN.md, M0-WP12, rows L0-01, L0-12,
// L0-13, L0-16, LF-12, LF-13).
//
//   node run/wp12.ts deploy                     # graft-wp12-load, graft-wp12-a/b/c (two-phase), bucket graft-wp12-stage
//   node run/wp12.ts smoke                      # one group: seed, acquire, push, conflict, feed poll, mint, clocks
//   node run/wp12.ts clocks [--n 20]            # member clock offsets (F3)
//   node run/wp12.ts l0-12 --kind grun [--seconds 600] [--stall 1800] [--phase a1|stall|idle|conflict|all] [--run X]
//   node run/wp12.ts l0-13 [--run X]
//   node run/wp12.ts l0-16 [--seconds 300] [--run X]
//   node run/wp12.ts lf-12 [--seconds 300] [--run X]
//   node run/wp12.ts lf-13 [--run X]
//   node run/wp12.ts l0-01 [--seconds 300] [--run X]
//   node run/wp12.ts delete
//
// Each phase writes results/raw/wp12-<row>-<run>-<phase>.json when it ends, and a rerun with the same --run skips a
// phase whose file exists, so a run that stops resumes at its next phase. Long runs go in a supervised service:
//   amp orb service start wp12-x --command "cd tests/m0 && node run/wp12.ts l0-12 --kind grun --run r1 >> results/raw/wp12-l0-12-r1.log 2>&1; sleep infinity"

import { existsSync, readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { a1, call, commitDist, deploy, dist, log, RAW, results, save, setup, sleep, spec, stamp, start, watch, wrangler, PREFIX, BUCKET, type Status } from "./wp12-lib.ts";
import { AGENT_KINDS, KINDS, VICTIM, changedPath, ref, scope, seed, session, victimWho, who, grunTask, type Kind } from "../src/wp12/fleet.ts";
import type { Lease, XCmd, XEv, XOut } from "../src/wp12/lease.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: false, options: {
  seconds: { type: "string" }, run: { type: "string" }, kind: { type: "string" }, stall: { type: "string" }, phase: { type: "string" },
  n: { type: "string" }, clients: { type: "string" }, agents: { type: "string" }, idle: { type: "string" }, only: { type: "string" }, learners: { type: "string" } } });
const RUN = (opt.run as string) ?? stamp();
const [cmd] = positionals;

type XRes = { out: XOut | null; code: number; commitMs: number; text?: string; ms?: number };
const x = (group: string, c: XCmd, key = "x") => call<XRes>("/x", { group, cmd: c, key });
const xmany = (group: string, cmds: { cmd: XCmd; key: string }[], conc = 20) => call<XRes[]>("/xmany", { group, cmds, conc }, 600_000);
const status = (group: string) => call<Status[]>("/status", { group });
const leaderOf = async (group: string) => (await status(group)).find((s) => s.role === "leader")?.site ?? null;
/** A phase runs once per run: its file is its checkpoint. */
async function phase<T>(name: string, fn: () => Promise<T>): Promise<T> {
  const path = `${RAW}/wp12-${name}.json`;
  if (existsSync(path)) { log(`phase ${name}: already done`); return JSON.parse(readFileSync(path, "utf8")) as T; }
  log(`phase ${name}: start`);
  const r = await fn();
  save(name, r);
  log(`phase ${name}: saved`);
  return r;
}

/** F3: offsets between members by round trip from each member to the other two, and from one edge to all three. */
async function clocks(group: string, n: number) {
  const r = await call<{ probes: { site: string; colo: string; rows: [string, number, number][] }[]; edge: [string, number, number][]; edgeColo: string }>("/clocks", { group, n }, 180_000);
  const pairs: Record<string, { rtt: ReturnType<typeof dist>; offset: ReturnType<typeof dist>; bound: number }> = {};
  const errors = r.probes.filter((p) => !p.rows).map((p) => JSON.stringify(p).slice(0, 200));
  for (const p of r.probes) {
    if (!p.rows) continue;
    for (const s of ["a", "b", "c"]) {
      const rows = p.rows.filter(([t]) => t === s);
      if (!rows.length) continue;
      // The true offset lies within offset ± rtt/2, so the tightest sample bounds it best: |offset| + rtt/2 of the
      // sample with the smallest round trip (`bound`). A sample with a slow round trip says little (its error is up to
      // rtt/2), so `offset` reports the samples within 1.5x of the minimum round trip as well as all of them.
      const best = rows.reduce((m, x) => (x[1] < m[1] ? x : m));
      const tight = rows.filter((x) => x[1] <= best[1] * 1.5);
      // The minimum of |offset| + rtt/2 over all samples is also an upper bound, and on a skewed path the tightest one.
      pairs[`${p.site}(${p.colo})>${s}`] = { rtt: dist(rows.map((x) => x[1])), offset: dist(rows.map((x) => Math.round(x[2]))),
        tightOffset: dist(tight.map((x) => Math.round(x[2]))), bound: Math.round(Math.min(...rows.map((x) => Math.abs(x[2]) + x[1] / 2))) } as never;
    }
  }
  const edge: Record<string, unknown> = {};
  for (const s of ["a", "b", "c"]) {
    const rows = r.edge.filter(([t]) => t === s);
    if (rows.length) { const best = rows.reduce((m, x) => (x[1] < m[1] ? x : m)); edge[s] = { rtt: dist(rows.map((x) => x[1])), offset: dist(rows.map((x) => Math.round(x[2]))), bound: Math.round(Math.abs(best[2]) + best[1] / 2) }; }
  }
  // The largest offset any sample with a round trip near its pair's minimum showed, and the largest bound.
  const maxAbs = Math.max(...Object.values(pairs).map((p) => { const t = (p as unknown as { tightOffset: ReturnType<typeof dist> }).tightOffset; return Math.max(Math.abs(t.p50 ?? 0), Math.abs(t.max ?? 0)); }));
  const maxBound = Math.max(...Object.values(pairs).map((p) => p.bound));
  return { at: new Date().toISOString(), group, n, errors, edgeColo: r.edgeColo, colos: r.probes.map((p) => `${p.site}:${p.colo}`), pairs, edge, maxAbsOffsetMs: maxAbs, maxBoundMs: maxBound };
}

async function smoke() {
  const group = `smoke-${RUN}`;
  log("setup", group);
  const st = await setup(group, [], seed("grun", 4, 2));
  log(st.map((s) => `${s.site}:${s.role}:${s.term}:${s.colo}`));
  const v = await x(group, { t: "Acquire", who: victimWho(), paths: [VICTIM.scope], refs: [], mode: "Exclusive" });
  log("victim", JSON.stringify(v.out));
  const own = await x(group, { t: "Acquire", who: who("grun", 0), paths: [scope(0)], refs: [], mode: "Exclusive" });
  log("own", JSON.stringify(own.out));
  const p1 = await x(group, { t: "Push", who: who("grun", 0), ref: ref("grun", 0), expect: null, value: "c1", changed: [changedPath(0, 1)], surface: "git" });
  log("own push", JSON.stringify(p1.out));
  const p2 = await x(group, { t: "Push", who: who("grun", 1), ref: ref("grun", 1), expect: null, value: "c1", changed: [VICTIM.path], surface: "git" });
  log("conflict push", JSON.stringify(p2.out));
  const p3 = await x(group, { t: "Push", who: who("human", 1), ref: "refs/heads/u1/task", expect: null, value: "c1", changed: [VICTIM.path], surface: "rest" });
  log("human push", JSON.stringify(p3.out));
  const m = await x(group, { t: "Merge", who: { cred: { t: "User", user: 1 } }, pr: 1, ref: "refs/heads/main", expect: null, value: "m1" });
  log("merge", JSON.stringify(m.out));
  log("mint", JSON.stringify(await call("/mint", { group, session: session("grun", 0) })));
  log("leases?paths", JSON.stringify(await call("/xread", { group, what: "leases", arg: ["pkg/**"] })).slice(0, 300));
  const dump = await call<any[]>("/leases", { group });
  log("dump", dump.map((d) => `${d.site}: applied ${d.applied}, ${d.leases?.length} leases`));
  log("clocks", JSON.stringify(await clocks(group, 5)).slice(0, 600));
  const stats = await call<{ voters: any[] }>("/w9stats", { group, learners: [], reset: false });
  log("rules", JSON.stringify(stats.voters.map((v) => ({ site: v.site, role: v.role, xRules: v.xRules, alarms: v.xAlarmCount, metaLoads: v.metaLoads, tables: v.tables }))));
}

switch (cmd) {
  case "deploy": deploy(); break;
  case "smoke": await smoke(); break;
  case "clocks": {
    const group = `clocks-${RUN}`;
    await setup(group, []);
    const r = await clocks(group, Number(opt.n ?? 20));
    save(`clocks-${RUN}`, r);
    console.log(JSON.stringify(r, null, 1));
    break;
  }
  case "delete": {
    // Stop writers first: purge through the load script, then delete the scripts, then empty the bucket through the R2
    // API (a run stopped mid-write left 27 objects in the third pass, and `r2 bucket delete` refuses a non-empty bucket).
    for (let i = 0; i < 20; i++) { const r = await call<{ deleted: number }>("/purge", { prefix: "" }).catch(() => ({ deleted: 0 })); if (!r.deleted) break; }
    for (const s of ["load", "a", "b", "c"]) try { wrangler(["delete", `${PREFIX}-${s}`, "--force"]); } catch (e) { console.error(String(e)); }
    const api = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets/${BUCKET}`;
    const auth = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` };
    for (let pass = 0; pass < 50; pass++) {
      const list = (await (await fetch(`${api}/objects?per_page=1000`, { headers: auth })).json()) as { result?: { key: string }[] };
      const keys = (list.result ?? []).map((o) => o.key);
      if (!keys.length) break;
      for (const k of keys) await fetch(`${api}/objects/${encodeURIComponent(k)}`, { method: "DELETE", headers: auth });
      console.log(`purged ${keys.length} objects through the R2 API`);
    }
    const del = (await (await fetch(api, { method: "DELETE", headers: auth })).json()) as { success: boolean; errors: unknown[] };
    console.log("bucket delete", JSON.stringify(del));
    if (!del.success) process.exitCode = 1;
    break;
  }
  default: {
    const { rows } = await import("./wp12-rows.ts");
    const fn = rows[cmd as keyof typeof rows];
    if (!fn) throw new Error("usage: node run/wp12.ts deploy | smoke | clocks | l0-12 | l0-13 | l0-16 | lf-12 | lf-13 | l0-01 | delete");
    await fn({ run: RUN, opt: opt as Record<string, string | undefined>, x, xmany, status, leaderOf, phase, clocks });
  }
}
void a1; void commitDist; void results; void spec; void start; void watch; void sleep; void AGENT_KINDS; void KINDS; void grunTask;
export type Ctx = { run: string; opt: Record<string, string | undefined>; x: typeof x; xmany: typeof xmany; status: typeof status; leaderOf: typeof leaderOf; phase: typeof phase; clocks: typeof clocks };
export type { Kind, Lease, XEv };
