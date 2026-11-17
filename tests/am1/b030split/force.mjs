// B0-30's short run (operator, 2026-11-16, option A; tests/am1/results/b030-split.md, "Proposal: a shorter B0-30 run
// from the split onward"). One driver for the steps after the split, each written to tests/am1/raw/$B030_RUN-force.jsonl
// as it happens and resumable from the phase it records there:
//   1. `split`: waits for the parent's first `MetaSplit` (a child in the leader's split state), then writes the run's
//      end, the split's simulated day plus AFTER_DAYS (8), to tests/am1/raw/b030-days, which load.mjs reads.
//   2. `sever`: waits for `MetaDropped`, then severs site c's parent member (LF-13's `/sever`, renewed every 60 s for
//      120 s at a time, so a crashed driver lifts it within two minutes) until the leader's `truncatedTo` passes
//      site c's applied by LAG_MARGIN (200), and lifts it. Site c must then install from the leader's source.
//   3. `place`: once site c's member reports an install running (its `/fold` `running.kind` is "install"), calls
//      `/place` on the parent's leader with `near_ms` 1, which makes every voter over the near bound, so only the
//      (ii)(a) gate stands between it and `Degraded` (or `/replace` on the third site when `/place` would pick site
//      c itself, or the leader is not the home). Pass: an answer naming site c behind, and no `config` entry in
//      the leader's log from the call's start to 30 s after it.
//   4. `install`: samples site c every 60 s until its install ends (applied past the source's `through` and no job):
//      its SQLite (the peak is judged against own − metadata + scratch), its `meta_dropped` mark, any
//      "install restarted" in its alarms, and whether it answered a metadata read or led while marked.
// Read-only apart from `/sever` and `/place`. Fails nothing on its own; every row carries what the result file judges.
// The second short run (b030r5, operator 2026-11-17): SEVER_AT=split severs site c as soon as `MetaSplit` commits,
// before `MetaDropped`, so it installs from a fold that never applied it (b030-split.md, "The gap"); END_AT=install
// sets the run's end (tests/am1/raw/b030-days) to the current day once c's install is done, so load.mjs stops there.
//   B030_RUN=b030r4 node tests/am1/b030split/force.mjs
//   B030_RUN=b030r5 SEVER_AT=split END_AT=install node tests/am1/b030split/force.mjs
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { log, member, sleep, statuses, voter } from "../hosted/lib.mjs";

const RUN = process.env.B030_RUN ?? "b030r4";
const OUT = `tests/am1/raw/${RUN}-force.jsonl`;
const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo;
const C = `repo:${REPO}`;
const SPEED = 18, DAY_S = 86_400 / SPEED;
const AFTER_DAYS = Number(process.env.AFTER_DAYS ?? 8);
const LAG_MARGIN = Number(process.env.LAG_MARGIN ?? 200);
const LAGGER = Number(process.env.LAGGER ?? 2);
const SEVER_AT = process.env.SEVER_AT ?? "dropped";
const END_AT = process.env.END_AT ?? "days";
const STOP = "tests/am1/raw/b030-STOP";
const row = (r) => { const v = { at: new Date().toISOString(), ...r }; appendFileSync(OUT, JSON.stringify(v) + "\n"); log(JSON.stringify(v).slice(0, 300)); };
const done = (phase) => existsSync(OUT) && readFileSync(OUT, "utf8").split("\n").some((l) => l.includes(`"phase":"${phase}"`) && l.includes(`"done":true`));
const leader = async () => (await statuses(C)).filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
const size = async (site) => (await voter(C, site, "/bench_seed", { b030: { size: true } }, 60_000).catch((e) => ({ v: { error: String(e).slice(0, 120) } }))).v;
const day = () => JSON.parse(readFileSync("tests/am1/raw/b030-state.json", "utf8")).active / DAY_S;

// 1. The split, and the run's end.
while (!done("split")) {
  if (existsSync(STOP)) process.exit(0);
  const l = await leader().catch(() => null);
  const v = l ? await size(l.site) : null;
  if (v?.split?.state?.child || v?.split?.state?.seed) {
    const d = day();
    writeFileSync("tests/am1/raw/b030-days", String(+(d + AFTER_DAYS).toFixed(3)) + "\n");
    row({ phase: "split", done: true, day: +d.toFixed(3), endsAt: +(d + AFTER_DAYS).toFixed(3), leader: l.site, dbBytes: v.dbBytes, split: v.split });
    break;
  }
  await sleep(30_000);
}

// 2. Site c past the leader's archive horizon.
if (!done("sever")) {
  for (;;) {
    if (existsSync(STOP)) process.exit(0);
    const l = await leader().catch(() => null);
    const v = l ? await size(l.site) : null;
    if (SEVER_AT === "split") { row({ phase: "dropped", skipped: "SEVER_AT=split", leader: l?.site, dropped: v?.split?.state?.dropped ?? null }); break; }
    if (v?.split?.state?.dropped) { row({ phase: "dropped", leader: l.site, dbBytes: v.dbBytes, slim: v.split.slim }); break; }
    await sleep(30_000);
  }
  const lagger = (await statuses(C)).find((s) => s.site === LAGGER);
  row({ phase: "sever", start: true, site: LAGGER, applied: lagger?.applied, doName: lagger?.doName });
  for (;;) {
    if (existsSync(STOP)) { await voter(C, LAGGER, "/sever", { for_ms: 0 }).catch(() => {}); process.exit(0); }
    await voter(C, LAGGER, "/sever", { for_ms: 120_000 }).catch((e) => row({ phase: "sever", error: String(e).slice(0, 120) }));
    const l = await leader().catch(() => null);
    const [lv, cv] = [l ? await size(l.site) : {}, (await member(C, LAGGER, "/status", undefined, 15_000, lagger?.doName).catch(() => ({ v: {} }))).v];
    row({ phase: "sever", leaderTruncatedTo: lv.truncatedTo, leaderApplied: lv.applied, laggerApplied: cv.applied });
    if (typeof lv.truncatedTo === "number" && typeof cv.applied === "number" && lv.truncatedTo > cv.applied + LAG_MARGIN) {
      await voter(C, LAGGER, "/sever", { for_ms: 0 });
      row({ phase: "sever", done: true, laggerApplied: cv.applied, leaderTruncatedTo: lv.truncatedTo });
      break;
    }
    await sleep(60_000);
  }
}

// 3. `/place` while site c installs: (ii)(a) must refuse it.
const installing = async () => (await member(C, LAGGER, "/fold", { audit: {} }, 30_000).catch(() => ({ v: {} }))).v?.running?.kind === "install";
if (!done("place")) {
  for (let i = 0; !(await installing()); i++) {
    if (existsSync(STOP) || i > 60) { row({ phase: "place", done: true, skipped: "no install seen in 60 min" }); break; }
    await sleep(60_000);
  }
  if (!done("place")) {
    const l = await leader();
    const before = (await member(C, l.site, "/status", undefined, 15_000)).v.last;
    // `/place` with `near_ms` 1 re-places the voter nearest the home (site 0). If that is the lagger, the gate would
    // ask only the third site, which is caught up, and a real re-placement would run, so the call is `/replace` on
    // the third site then: the same `replace_on` and the same gate, needing the lagger.
    const rtt = (await member(C, 0, "/rtt", undefined, 15_000).catch(() => ({ v: [] }))).v ?? [];
    const ms = Object.fromEntries((Array.isArray(rtt) ? rtt : []).map((x) => [x[1]?.peer, x[1]?.median_ms]).filter(([p]) => p !== undefined));
    const nearest = [1, 2].filter((s) => ms[s] !== undefined).sort((a, b) => ms[a] - ms[b])[0];
    const third = [0, 1, 2].find((s) => s !== l.site && s !== LAGGER);
    const [path, body] = nearest !== undefined && nearest !== LAGGER && l.site === 0 ? ["/place", { near_ms: 1 }] : ["/replace", { site: third }];
    const t0 = Date.now();
    const r = await voter(C, l.site, path, body, 90_000).catch((e) => ({ v: { error: String(e).slice(0, 200) } }));
    await sleep(Math.max(0, 30_000 - (Date.now() - t0)));
    const ents = (await member(C, l.site, `/log?from=${before + 1}&n=500`, undefined, 30_000).catch(() => ({ v: [] }))).v ?? [];
    const configs = ents.filter((e) => e.kind === "config").map((e) => ({ index: e.index, committed: e.committed }));
    row({ phase: "place", done: true, leader: l.site, path, body, rtt: ms, answer: r.v, logFrom: before + 1, entries: ents.length, configs, pass: configs.length === 0 && JSON.stringify(r.v).includes("behind") });
  }
}

// 4. Site c's install, to its end.
let peak = 0, own = null, through = null;
for (;;) {
  if (existsSync(STOP) && !existsSync("tests/am1/raw/b030-load.done")) break;
  // A member reply with no body (07:17Z and 08:06Z on b030r5) left a sample undefined and killed the driver.
  const v = (await size(LAGGER)) ?? {};
  const f = (await member(C, LAGGER, "/fold", { audit: {} }, 30_000).catch(() => ({ v: {} }))).v ?? {};
  const st = (await member(C, LAGGER, "/status", undefined, 15_000).catch(() => ({ v: {} }))).v ?? {};
  own ??= v.dbBytes;
  peak = Math.max(peak, v.dbBytes ?? 0);
  const restarts = (v.alarms ?? []).filter((a) => a.includes("install restarted")).length;
  const running = f?.running?.kind ?? null;
  if (running === "install") through = f.running.through;
  row({ phase: "install", dbBytes: v.dbBytes, peak, applied: v.applied, role: st.role, running, through,
    restarts, sqliteFull: v.sqliteFull, alarms: (v.alarms ?? []).slice(0, 3) });
  // Done once no job runs and the member applied past the last install's `through`.
  if (running === null && through !== null && typeof v.applied === "number" && v.applied >= through) {
    row({ phase: "install", done: true, own, peak, applied: v.applied });
    if (END_AT === "install") {
      const d = day();
      writeFileSync("tests/am1/raw/b030-days", String(+d.toFixed(3)) + "\n");
      row({ phase: "end", done: true, day: +d.toFixed(3), why: "END_AT=install" });
    }
    break;
  }
  await sleep(60_000);
}
log("force: done");
