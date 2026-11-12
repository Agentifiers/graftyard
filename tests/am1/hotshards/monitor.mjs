// hot-shards' monitor on graft-m3c: every MONITOR_S (default 60), one row in tests/am1/raw/hotshards/monitor.jsonl
// with each member's role, applied index, log bytes, the log above the last Promoted (`unpromotedBytes`), the
// archived and promoted indexes and archive steps (`/bench_seed {"b030": {"size": true}}`), and each warm site's first
// shard's status (`/_bed/shard_status`), and every CATCHUP_EVERY rows (default 5; 0 for none) a catch-up report from
// each site's first shard (`/_bed/catchup?shard=N&wait=60`).
// Runs until tests/am1/raw/hotshards/STOP exists. Needs tests/am1/lf13/env-m3c.sh sourced, HOT_CHAIN and HOT_ID.
//   node tests/am1/hotshards/monitor.mjs
import { appendFileSync, existsSync, mkdirSync } from "node:fs";
import { bedGet, log, member, sleep } from "../hosted/lib.mjs";

const CHAIN = process.env.HOT_CHAIN;
const ID = process.env.HOT_ID;
const OUT = "tests/am1/raw/hotshards/monitor.jsonl";
const STOP = "tests/am1/raw/hotshards/STOP";
const EVERY = Number(process.env.MONITOR_S ?? 60) * 1000;
const CATCHUP_EVERY = Number(process.env.CATCHUP_EVERY ?? 5);
let n = 0;
mkdirSync("tests/am1/raw/hotshards", { recursive: true });

while (!existsSync(STOP)) {
  const at = Date.now();
  const members = await Promise.all([0, 1, 2].map(async (s) => {
    try {
      const v = (await member(CHAIN, s, "/bench_seed", { b030: { size: true } }, 25_000)).v ?? {};
      return { site: s, role: v.role, applied: v.applied, logBytes: v.logBytes, logFirst: v.logFirst, unpromoted: v.unpromotedBytes,
        promotedTo: v.promotedTo, archivedTo: v.archivedTo, archiveSteps: v.archiveSteps, db: v.dbBytes, alarms: v.alarms?.slice(0, 2) };
    } catch (e) { return { site: s, error: String(e).slice(0, 100) }; }
  }));
  const shards = await Promise.all([0, 1, 2].map(async (s) => {
    try { const r = await bedGet(`/_bed/shard_status?repo=${ID}&shard=${s}`); return { site: s, status: r.status, v: r.body.slice(0, 300) }; }
    catch (e) { return { site: s, error: String(e).slice(0, 80) }; }
  }));
  // Every CATCHUP_EVERY rows, a catch-up on each site's first shard (at most 60 s each, `warming` past it): its report
  // has the entries and packs it applied, `held_max` (inline bytes held at once), and its laps.
  let catchups;
  if (CATCHUP_EVERY && n++ % CATCHUP_EVERY === 0) {
    catchups = await Promise.all([0, 1, 2].map(async (s) => {
      const t = Date.now();
      try {
        const r = await bedGet(`/_bed/catchup?repo=${ID}&shard=${s}&wait=60`);
        let v = r.body;
        try { const j = JSON.parse(r.body); v = { entries: j.entries, packs: j.packs, held_max: j.held_max, cursor: j.cursor?.chains, ms: j.ms, rehydrated: j.rehydrated, warming: j.warming }; } catch {}
        return { site: s, status: r.status, wall: Date.now() - t, v: typeof v === "string" ? v.slice(0, 200) : v };
      } catch (e) { return { site: s, error: String(e).slice(0, 80) }; }
    }));
  }
  const v = { at: new Date(at).toISOString(), members, shards, catchups };
  appendFileSync(OUT, JSON.stringify(v) + "\n");
  const l = members.find((m) => m.role === "Leader");
  log(`monitor leader ${l?.site} applied ${l?.applied} log ${((l?.logBytes ?? 0) / 1e9).toFixed(3)} GB above-promoted ${((l?.unpromoted ?? 0) / 1e9).toFixed(3)} GB promoted ${l?.promotedTo} archived ${l?.archivedTo} steps ${l?.archiveSteps}`);
  await sleep(Math.max(5_000, EVERY - (Date.now() - at)));
}
