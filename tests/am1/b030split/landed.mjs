// Did b030r6's "http 0" pushes land? (operator, 2026-11-17.) The 41 failures before 17:30:46Z were logged without
// their commit, so this counts instead: every `branchMoves` row on refs/heads/b030/* in the committed log, by the
// leader's entry time, read one entry a call from a follower (each entry carries its ~320 KB inline pack, and reading
// 100 or 400 at a time from the leader took its isolate past memory at 17:36-17:37Z). In a window, moves minus the
// driver's acknowledged pushes is how many unacknowledged pushes committed: about the window's "http 0" count if they
// landed, about 0 if they did not. Writes tests/am1/raw/b030r6-landed.jsonl (one row per entry: index, at, moves).
//   node tests/am1/b030split/landed.mjs SITE DO_NAME FROM_ISO TO_ISO
import { appendFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";
const [site, name, fromIso, toIso] = process.argv.slice(2);
const C = name.split("#")[0], S = Number(site), OUT = "tests/am1/raw/b030r6-landed.jsonl";
const one = async (i) => (await member(C, S, `/log?from=${i}&n=1`, undefined, 30_000, name)).v?.[0];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let lo = (await one(1)).index, hi = (await member(C, S, "/status", undefined, 15_000, name)).v.commit;
const from = Date.parse(fromIso), to = Date.parse(toIso);
while (lo < hi) { const m = (lo + hi) >> 1; const e = await one(m); if (e.at < from) lo = m + 1; else hi = m; await sleep(100); }
for (let i = lo; ; i++) {
  const e = await one(i);
  if (!e || e.at > to) break;
  appendFileSync(OUT, JSON.stringify({ index: e.index, at: e.at, moves: (e.branchMoves ?? []).filter((m) => String(m[1]).includes("b030/")).map((m) => m.slice(1)) }) + "\n");
  await sleep(150);
}
console.log("done", lo);
