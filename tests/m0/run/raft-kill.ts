// Leader kill under load (M0-WP12's follow-on step-down, rerun on M0-WP4's copy). `--groups` groups led on site A take
// `--load` writes/s in total. Then every group's leader on A is reset (`ctx.abort`) and every call to or from A fails
// for 15 s, as M0-WP12 killed a leader. For 60 s after the kill the runner samples each group's leader every second and
// counts leader changes; then it reads the members' logs for each `STEP_DOWN` and its reason.
//
//   RAFT_PREFIX=graft-wp4 node run/raft-kill.ts --run NAME [--groups 30] [--load 100] [--after 60] [--prefix kill]
// `--prefix min3` names the groups `min3-*`, which the member treats as the F37 variant (3 failed calls per follower).

import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { createLedFromA, raft, RAFT_URL } from "./raft.ts";

const { values: opt } = parseArgs({ options: { groups: { type: "string", default: "30" }, load: { type: "string", default: "100" },
  after: { type: "string", default: "60" }, prefix: { type: "string", default: "kill" }, run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) } } });
const ROOT = new URL("../", import.meta.url).pathname;
const OUT = `${ROOT}results/raw/kill-${opt.run}.json`;
// A supervised service restarts its command on exit; a finished run must not be overwritten.
if (existsSync(OUT)) { console.log(`${OUT} exists; not rerunning`); setInterval(() => {}, 3_600_000); await new Promise(() => {}); }
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const groups = Array.from({ length: Number(opt.groups) }, (_, i) => `${opt.prefix}-${opt.run}-g${i}`);
type St = { site: string; role?: string; term?: number; error?: string };
const leaderOf = (ms: St[]) => ms.filter((m) => m.role === "leader").sort((x, y) => (y.term ?? 0) - (x.term ?? 0))[0]?.site ?? null;

await raft("/faults", { plan: { rules: [] } });
await sleep(2_500);
for (let i = 0; i < groups.length; i += 10) await Promise.all(groups.slice(i, i + 10).map((g) => createLedFromA(g)));
log(`${groups.length} groups led on a`);

// Load: 20 callers, 5 workers each at `load / 100` writes/s per worker, spread over all groups.
let loading = true;
const ops: [number, number, string, boolean, string?][] = [];
const callers = Array.from({ length: 20 }, async (_, i) => {
  while (loading) {
    const r = await raft<{ ops?: typeof ops; error?: string }>("/bench", { group: groups[0], groups, n: 100, concurrency: 5,
      ratePerWorker: Number(opt.load) / 100, bytes: 1024, ops: true, colo: i % 2 ? "kill-second-colo" : undefined }, 120_000)
      .catch((e) => ({ error: String(e) }));
    if (r.ops) ops.push(...r.ops); else log(`load call failed: ${r.error?.slice(0, 120)}`);
  }
});

const samples: { at: number; leaders: (string | null)[] }[] = [];
let sampling = true;
const sampler = (async () => {
  while (sampling) {
    const at = Date.now();
    const st = await raft<Record<string, St[]>>("/statusMany", { groups }, 30_000).catch(() => null);
    if (st) samples.push({ at, leaders: groups.map((g) => leaderOf(st[g] ?? [])) });
    await sleep(Math.max(0, 1_000 - (Date.now() - at)));
  }
})();

await sleep(20_000);
const killAt = Date.now();
await Promise.all(groups.map((group) => raft("/reset", { group, sites: ["a"] }).catch(() => {})));
await raft("/faults", { plan: { rules: [{ target: "call", mode: "error", rate: 1, match: ">a:" }, { target: "call", mode: "error", rate: 1, match: "a>" }],
  from: killAt - 2_000, until: killAt + 15_000 } });
log("killed a in every group; calls to and from a fail for 15 s");
await sleep(Number(opt.after) * 1000);
loading = false; sampling = false;
await Promise.all(callers); await sampler;
await raft("/faults", { plan: { rules: [] } });

const logs = await Promise.all(groups.map((group) => raft<{ site: string; log?: string[] }[]>("/stats", { group, reset: false }, 30_000)
  .then((ms) => ms.map((m) => ({ site: m.site, log: (m.log ?? []).filter((l) => Number(l.slice(0, 13)) >= killAt - 5_000 &&
    /STEP_DOWN|won term|ready|vote|ack |append to|hand|transfer/.test(l)) })), (e) => [{ site: "?", log: [String(e)] }])));
const perGroup = groups.map((group, gi) => {
  const seq = samples.filter((s) => s.at >= killAt && s.at <= killAt + Number(opt.after) * 1000).map((s) => ({ at: s.at, leader: s.leaders[gi] }));
  const changes: { ms: number; to: string }[] = [];
  let prev: string | null = "a";
  for (const s of seq) if (s.leader && s.leader !== prev) { changes.push({ ms: s.at - killAt, to: s.leader }); prev = s.leader; }
  const lines = logs[gi].flatMap((m) => m.log.map((l) => `${m.site} ${l}`));
  return { group, changes: changes.length, changeTimes: changes, stepDowns: lines.filter((l) => /STEP_DOWN (in|held)/.test(l)), log: logs[gi] };
});
const after = ops.filter((o) => o[0] >= killAt);
const summary = { row: "kill", run: opt.run, at: new Date().toISOString(), url: RAFT_URL, groups: groups.length, prefix: opt.prefix, load: Number(opt.load), killAt,
  changesHistogram: perGroup.reduce((h, g) => ({ ...h, [g.changes]: (h[g.changes] ?? 0) + 1 }), {} as Record<number, number>),
  groupsWithSecondChange: perGroup.filter((g) => g.changes >= 2).length, stepDownTrips: perGroup.reduce((n, g) => n + g.stepDowns.filter((l) => /STEP_DOWN in/.test(l)).length, 0),
  opsAfterKill: after.length, failedAfterKill: after.filter((o) => !o[3]).length, perGroup };
mkdirSync(`${ROOT}results/raw`, { recursive: true });
writeFileSync(OUT, JSON.stringify(summary, null, 2));
log(JSON.stringify({ ...summary, perGroup: undefined }));
