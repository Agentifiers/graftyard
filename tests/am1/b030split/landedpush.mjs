// b030r6's "http 0" pushes logged with their commit (tests/am1/raw/b030r6-fetchfail.jsonl rows with `commit`): for
// each burst, the committed log on a follower is read one entry a call (never more: 100-400 a call reset the leader's
// isolate at 17:36Z) from the burst to 60 s after it, and each pushed commit is looked for as a b030 branch move.
// Prints one line per push (committed at index and time, or not found) and a total.
//   node tests/am1/b030split/landedpush.mjs SITE DO_NAME
import { readFileSync } from "node:fs";
import { member } from "../hosted/lib.mjs";
const [site, name] = process.argv.slice(2);
const C = name.split("#")[0], S = Number(site);
const pushes = readFileSync("tests/am1/raw/b030r6-fetchfail.jsonl", "utf8").trim().split("\n").map(JSON.parse).filter((r) => r.commit && r.ms !== undefined);
const one = async (i) => (await member(C, S, `/log?from=${i}&n=1`, undefined, 30_000, name)).v?.[0];
const first = async (t) => { let lo = (await one(1)).index, hi = (await member(C, S, "/status", undefined, 15_000, name)).v.commit; while (lo < hi) { const m = (lo + hi) >> 1; if ((await one(m)).at < t) lo = m + 1; else hi = m; } return lo; };
const bursts = [];
for (const p of pushes) { const t = Date.parse(p.at); const b = bursts.find((b) => Math.abs(b.t - t) < 30_000); b ? b.p.push(p) : bursts.push({ t, p: [p] }); }
let found = 0;
for (const b of bursts) {
  const want = new Map(b.p.map((p) => [p.commit, p]));
  const hit = new Map();
  for (let i = await first(b.t - 2_000); ; i++) {
    const e = await one(i); if (!e || e.at > b.t + 60_000) break;
    for (const m of e.branchMoves ?? []) if (want.has(m[3])) hit.set(m[3], `committed at ${e.index} ${new Date(e.at).toISOString().slice(11, 23)}Z from ${String(m[2]).slice(0, 8)}${m[2] === want.get(m[3]).old ? " (the sent old tip)" : " (NOT the sent old tip)"}`);
  }
  for (const [c, p] of want) { console.log(p.ref, c.slice(0, 8), hit.get(c) ?? "NOT FOUND"); if (hit.has(c)) found++; }
}
console.log(JSON.stringify({ pushes: pushes.length, bursts: bursts.length, committed: found }));
