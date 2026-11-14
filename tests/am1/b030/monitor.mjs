// B0-30's monitor: every MONITOR_S (default 900 s; the brief asks for at least hourly), one row in
// tests/am1/raw/b030-monitor.jsonl with, for each of the three members, its SQLite bytes (`databaseSize` as an f64),
// log rows and bytes, `request` rows, archive ranges and `sqliteFull`; the archive objects of this chain in the two
// record buckets (count and bytes, listed through the Cloudflare API, token on stdin); `SQLITE_FULL` and
// "database or disk is full" in Workers Logs of the four site scripts over the interval; whether any member passed
// CHILD_GROUP_AT (8 GiB); and graft-m1's cost since the run started (tests/am1/b030/usage.py) plus R2's estimate.
// Writes tests/am1/raw/b030-STOP when the cost to date passes STOP_USD, so the load ends. Resumable: it only appends.
//   node tests/am1/b030/monitor.mjs
import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { log, member, sleep } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const CHAIN = `repo:${REPO.repo}`;
const OUT = "tests/am1/raw/b030-monitor.jsonl";
const STOP = "tests/am1/raw/b030-STOP";
const EVERY = Number(process.env.MONITOR_S ?? 900) * 1000;
const STOP_USD0 = Number(process.env.STOP_USD ?? 88);
// A new cap mid-run: tests/am1/raw/b030-stop-usd holds a number that replaces STOP_USD on the next row.
const stopUsd = () => { try { return Number(readFileSync("tests/am1/raw/b030-stop-usd", "utf8").trim()) || STOP_USD0; } catch { return STOP_USD0; } };
// Spent before the load started (the seed and setup, 06:30Z to the load's start), from usage.py; added to cost.
const SINCE = process.env.B030_SINCE ?? "2026-11-10T06:30:00Z";
const CHILD_GROUP_AT = 8 * 2 ** 30;
const SITES = ["graft-m1-site-a", "graft-m1-site-b", "graft-m1-site-c", "graft-m1-site-d"];
const BUCKETS = ["graft-m1-packs-a", "graft-m1-packs-b"];
const cf = (path, input = "") => JSON.parse(execFileSync("curl", ["-s", "-H", "@-", ...(input ? ["-H", "content-type: application/json", "--data-binary", "@/dev/fd/3"] : []),
  `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/${path}`],
  { input: `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}\n`, stdio: ["pipe", "pipe", "pipe", ...(input ? ["pipe"] : [])], maxBuffer: 64 << 20 }).toString());

function bucket(name) {
  let cursor = null, n = 0, bytes = 0, first = null, last = null;
  for (let page = 0; page < 1000; page++) {
    const r = cf(`r2/buckets/${name}/objects?prefix=archive/${CHAIN}/&per_page=1000${cursor ? `&cursor=${cursor}` : ""}`);
    if (!r.success) return { error: JSON.stringify(r.errors).slice(0, 200) };
    for (const o of r.result) { n++; bytes += o.size; first ??= o.key.split("/")[2]?.split("-").slice(0, 2).join("-"); last = o.key.split("/")[2]?.split("-").slice(0, 2).join("-"); }
    cursor = r.result_info?.cursor;
    if (!r.result_info?.is_truncated || !cursor) break;
  }
  return { objects: n, bytes, first, last };
}
function logsHits(from, to) {
  const out = {};
  for (const s of SITES) {
    out[s] = 0;
    for (const needle of ["SQLITE_FULL", "database or disk is full"]) {
      try {
        const v = execFileSync("python3", ["tests/am1/b030/logs.py", s, String(from / 1000), String(to / 1000), needle], { maxBuffer: 16 << 20 }).toString();
        out[s] += v.split("\n").filter(Boolean).length;
      } catch (e) { out[s] = `error ${String(e).slice(0, 60)}`; }
    }
  }
  return out;
}
function usage(from) {
  try { return JSON.parse(execFileSync("python3", ["tests/am1/b030/usage.py", from, new Date().toISOString()]).toString()); }
  catch (e) { return { error: String(e).slice(0, 120) }; }
}
// graft-m1's shard containers over the same window (tests/fanout/containers.py), which usage.py leaves out; a run on
// standard-4 shards counts them toward STOP_USD.
function containers(from) {
  try {
    return JSON.parse(execFileSync("python3", ["tests/fanout/containers.py", from, new Date().toISOString()], { env: { ...process.env, INSTALL: "graft-m1" } }).toString());
  } catch (e) { return { error: String(e).slice(0, 120) }; }
}

let lastAt = existsSync(OUT) ? Date.parse(readFileSync(OUT, "utf8").trim().split("\n").at(-1) ? JSON.parse(readFileSync(OUT, "utf8").trim().split("\n").at(-1)).at : 0) : Date.now() - EVERY;
for (;;) {
  const at = Date.now();
  const members = [];
  for (const s of [0, 1, 2]) {
    try { members.push({ site: s, ...(await member(CHAIN, s, "/bench_seed", { b030: { size: true } }, 25_000)).v }); }
    catch (e) { members.push({ site: s, error: String(e).slice(0, 120) }); }
  }
  for (const m of members) delete m.meta;
  const buckets = Object.fromEntries(BUCKETS.map((b) => [b, bucket(b)]));
  const full = logsHits(lastAt, at);
  const u = usage(SINCE);
  const k = containers(SINCE);
  // R2: two Class A puts per archive object and two per promoted pack, plus storage pro rata; small next to DO.
  const objs = Object.values(buckets).reduce((a, b) => a + (b.objects ?? 0), 0);
  const r2 = objs * 2 * 4.5e-6 + (Object.values(buckets).reduce((a, b) => a + (b.bytes ?? 0), 0) / 1e9) * 0.015 * ((at - Date.parse(SINCE)) / (30 * 86_400_000));
  const cost = (u.cost ?? NaN) + r2 + (k.containers_usd ?? 0);
  const state = existsSync("tests/am1/raw/b030-state.json") ? JSON.parse(readFileSync("tests/am1/raw/b030-state.json", "utf8")) : {};
  const day = state.active ? state.active / (86_400 / Number(process.env.B030_SPEED ?? 18)) : 0;
  const maxDb = Math.max(...members.map((m) => m.dbBytes ?? 0));
  const v = { at: new Date(at).toISOString(), day: +day.toFixed(3), members, buckets, sqliteFullLogs: full,
    sqliteFullCount: members.reduce((a, m) => a + (m.sqliteFull ?? 0), 0) + Object.values(full).reduce((a, n) => a + (typeof n === "number" ? n : 0), 0),
    childGroupAtReached: maxDb >= CHILD_GROUP_AT, maxDbBytes: maxDb, usage: u, containers: k, r2Estimate: +r2.toFixed(2), costToDate: +cost.toFixed(2),
    load: { pushes: state.pushes, pushBytes: state.pushBytes, tasks: state.tasks, checks: state.checks, provenance: state.provenance, batches: state.batches, batchFail: state.batchFail } };
  appendFileSync(OUT, JSON.stringify(v) + "\n");
  log(`monitor day ${v.day} db ${members.map((m) => ((m.dbBytes ?? 0) / 1e9).toFixed(3)).join("/")} GB log ${members.map((m) => ((m.logBytes ?? 0) / 1e9).toFixed(3)).join("/")} GB archived ${objs} full ${v.sqliteFullCount} cost $${v.costToDate}`);
  const cap = stopUsd();
  if (cost > cap && !existsSync(STOP)) { writeFileSync(STOP, `cost ${cost} past ${cap} at ${v.at}\n`); log(`STOP: cost ${cost}`); }
  lastAt = at;
  if (existsSync("tests/am1/raw/b030-load.done") && existsSync(STOP + ".monitor")) break;
  await sleep(Math.max(10_000, EVERY - (Date.now() - at)));
}
