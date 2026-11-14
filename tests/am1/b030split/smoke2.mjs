// b030-split smoke 2 on graft-m1, with no keeper: a split and its slim pass on a fresh group whose leader alone writes,
// as in B0-30's load, and every leaderless gap and STEP_DOWN while they run. Before the 30-day rerun it shows the two
// fixes (`decisions/DECISIONS.md`, "A leader paces its own background writes, and a leaderless voter with such work
// due acts after 20 s"): paced `MetaSlim` entries, and a voter that campaigns for leader-only work after 20 s.
//
// 1. A fresh repository (tests/am1/b030/mkrepo.mjs NAME) seeded to SEED bytes (tests/am1/b030/seed.mjs), then each
//    member's `SPLIT_AT` set just under that (`/bench_seed {"splitAt"}`), so the split runs as at 7 GiB, smaller. The
//    group has no leader then, so a voter must elect one for the split on its own; then the snapshot is forced.
// 2. Every 5 s until the slim pass is done: which member leads (by `/status`), so each leaderless gap is measured.
// 3. Once the slim pass has run CUT_AFTER_S, the leader is cut off for CUT_S (`/sever`), as a site that stops
//    answering: it stops leading by check-quorum, nobody calls the group, and a voter must campaign for the slim work
//    on its own within about 2 x ELECTION_AFTER of its promise lapsing.
// 4. At the end: the gaps, StepDown and keep-led lines from Workers Logs, and the comment bodies of a sample of keys
//    read through the parent before and after, which must match.
// Rows go to tests/am1/raw/b030split-smoke2.jsonl.
//   node tests/am1/b030split/smoke2.mjs
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { leaderOf, log, member, row, sleep, statuses } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const C = `repo:${REPO.repo}`;
const OUT = `tests/am1/raw/b030split-${process.env.TAG ?? "smoke2"}.jsonl`;
const SAMPLE = 100;
const sha = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);
const seeded = JSON.parse(readFileSync("tests/am1/raw/b030-seed.json", "utf8"));
const rows = seeded.rows;

async function bodies(keys) {
  const q = { q: { rows: { keys: keys.map((k) => ["comment", [{ Int: k }]]) } } };
  let v;
  for (let i = 0; i < 20 && !Array.isArray(v?.value); i++) {
    if (i) await sleep(10_000);
    v = (await member(C, await leaderOf(C), "/read", q, 120_000).catch((e) => ({ v: { error: String(e) } }))).v;
  }
  if (!Array.isArray(v?.value)) throw new Error(`keys read: ${JSON.stringify(v).slice(0, 300)}`);
  const out = {};
  for (const [, key, cols] of v.value) if (cols) out[key[0].Int] = Buffer.from(cols[0].Blob).toString("latin1");
  return out;
}
const size = async (s) => (await member(C, s, "/bench_seed", { b030: { size: true } }, 60_000).catch(() => ({ v: {} }))).v;

const keys = Array.from({ length: SAMPLE }, (_, i) => 2 ** 40 + Math.floor((i * rows) / SAMPLE));
// The seeded group has had no request since its create, so it has no leader. SPLIT_AT goes to every member with
// none: only the keep-led rule can elect one now, and the time it takes is the first leaderless gap.
const sizes = await Promise.all([0, 1, 2].map(size));
const at = Math.min(...sizes.map((s) => s.dbBytes)) - 1e6;
const t0 = Date.now();
for (const s of [0, 1, 2]) log(`splitAt ${s}: ${JSON.stringify((await member(C, s, "/bench_seed", { splitAt: at })).v)}`);
row(OUT, { kind: "start", at: new Date(t0).toISOString(), splitAt: at, dbBytes: sizes.map((s) => s.dbBytes) });
let first = null;
while (!first && Date.now() - t0 < 300_000) {
  first = (await statuses(C)).find((s) => s.role === "leader");
  if (!first) await sleep(2_000);
}
const electS = Math.round((Date.now() - t0) / 1000);
log(first ? `elected with no request: site ${first.site} after ${electS} s` : `no leader 300 s after splitAt`);
row(OUT, { kind: "elect", s: electS, site: first?.site ?? null });
// The split seeds at the newest committed metadata snapshot: one now, on the leader.
log(`force: ${JSON.stringify((await member(C, first?.site ?? 0, "/bench_seed", { force: 1 })).v)}`);
const b0 = await bodies(keys);
log(`read before: ${Object.keys(b0).length} of ${SAMPLE}`);

// Leaderless gaps, from each member's role every 5 s.
const gaps = [];
const CUT_AFTER_S = Number(process.env.CUT_AFTER_S ?? 180), CUT_S = Number(process.env.CUT_S ?? 15);
let since = null, last = "", done = false, slimFrom = null, cut = null;
while (!done) {
  const st = await statuses(C);
  const led = st.find((s) => s.role === "leader");
  const now = Date.now();
  if (led) { if (since) { gaps.push({ from: new Date(since).toISOString(), s: Math.round((now - since) / 1000) }); log(`leader again on ${led.site} after ${Math.round((now - since) / 1000)} s`); since = null; } }
  else since ??= now;
  if (led) {
    const v = await size(led.site);
    const sp = JSON.stringify({ state: v.split?.state && { dropped: v.split.state.dropped, seed: v.split.state.seed?.[0] }, slim: v.split?.slim && { done: v.split.slim.done, table: v.split.slim.table } });
    if (sp !== last) { log(`split ${sp} dbBytes ${v.dbBytes} keepLed ${v.keepLed}`); row(OUT, { kind: "step", at: new Date().toISOString(), split: v.split, dbBytes: v.dbBytes }); last = sp; }
    done = v.split?.slim?.done === true;
    if (v.split?.slim && !done) slimFrom ??= now;
    if (!cut && slimFrom && now - slimFrom >= CUT_AFTER_S * 1000) {
      cut = { at: new Date().toISOString(), site: led.site, v: (await member(C, led.site, "/sever", { for_ms: CUT_S * 1000 }).catch((e) => ({ v: String(e) }))).v };
      log(`cut leader ${led.site} for ${CUT_S} s: ${JSON.stringify(cut.v)}`);
      row(OUT, { kind: "cut", ...cut });
    }
  }
  if (now - t0 > 4 * 3600_000) { log("no end in 4 h"); break; }
  await sleep(5_000);
}
if (since) gaps.push({ from: new Date(since).toISOString(), s: Math.round((Date.now() - since) / 1000), open: true });
const t1 = Date.now();
const b1 = await bodies(keys);
let same = 0;
for (const [k, v] of Object.entries(b0)) if (b1[k] === v) same++;
// StepDown and keep-led lines on each site in the window.
const logs = {};
for (const s of ["a", "b", "c"]) {
  const out = execFileSync("python3", ["tests/am1/b030/logs.py", `graft-m1-site-${s}`, String(t0 / 1000), String(t1 / 1000)], { env: { ...process.env, STEP: "1800" }, encoding: "utf8", maxBuffer: 1 << 28 });
  const ev = out.trim().split("\n").filter(Boolean).map((l) => JSON.parse(l));
  logs[s] = { stepDowns: ev.filter((e) => e.msg.includes("StepDown")).length, keepLed: ev.filter((e) => e.msg.startsWith("keep led")).map((e) => e.msg.slice(0, 120)) };
}
const after = await Promise.all([0, 1, 2].map(size));
const sum = { at: new Date().toISOString(), min: Math.round((t1 - t0) / 60_000), electS, cut, gaps, longestGapS: Math.max(0, ...gaps.map((g) => g.s)), same, of: Object.keys(b0).length, logs, before: sizes.map((s) => s.dbBytes), after: after.map((s) => s.dbBytes), sqliteFull: after.map((s) => s.sqliteFull) };
row(OUT, { kind: "summary", ...sum });
log(`smoke2 ${JSON.stringify(sum)}`);
