// B0-30's end check: a sample of archived ranges read back from both record buckets and checked by the leader
// (`/bench_seed {"b030": {"verify": {key}}}`: SHA-256 against the key, header, contiguous indexes, the `prev` chain),
// as M0's run checked its archives (tests/m0/results/m0-wp9.md, B0-30), and `RestoredTo` lookups
// (`{"b030": {"at": t}}`) at times inside sampled ranges, each checked against the range's own entries.
// Writes tests/am1/raw/b030-verify.jsonl and prints a summary.
//   N=80 node tests/am1/b030/verify.mjs
import { appendFileSync, readFileSync } from "node:fs";
import { leaderOf, log, member, voter } from "../hosted/lib.mjs";

const C = `repo:${JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo}`;
const OUT = "tests/am1/raw/b030-verify.jsonl";
const N = Number(process.env.N ?? 80);
let s = await leaderOf(C);
// A leader change mid-run answers notLeader or nothing: look the leader up again and retry, up to 6 times.
async function ask(body, ms) {
  for (let t = 0; ; t++) {
    const v = (await member(C, s, "/bench_seed", body, ms).catch(() => ({}))).v;
    if (v && !v.error && !v.notLeader) return v;
    if (t >= 5) return v ?? { error: "no answer" };
    await new Promise((r) => setTimeout(r, 5_000));
    s = await leaderOf(C);
  }
}
const all = [];
for (let after = 0; ;) {
  const v = await ask({ b030: { archives: { after, n: 500 } } }, 60_000);
  if (!v?.archives?.length) break;
  all.push(...v.archives);
  after = v.archives.at(-1).to;
}
// Evenly spaced over the whole run, plus the first and the last.
const pick = [...new Set([0, all.length - 1, ...Array.from({ length: N }, (_, k) => Math.floor((k * all.length) / N))])]
  .filter((k) => k >= 0 && k < all.length).map((k) => all[k]);
let ok = 0, bytes = 0, entries = 0;
const bad = [];
for (const a of pick) {
  const v = await ask({ b030: { verify: { key: a.key } } }, 120_000);
  appendFileSync(OUT, JSON.stringify({ kind: "verify", ...v }) + "\n");
  if (v.ok) { ok++; bytes += v.copies[0].bytes; entries += v.copies[0].entries; } else bad.push(a.key);
}
// RestoredTo: a time strictly inside each of 10 sampled ranges must resolve to an entry of that range.
let rOk = 0;
const rBad = [];
for (const a of pick.filter((_, k) => k % Math.max(1, Math.floor(pick.length / 10)) === 0).slice(0, 10)) {
  const v = await ask({ b030: { at: a.toAt } }, 120_000);
  appendFileSync(OUT, JSON.stringify({ kind: "at", range: [a.from, a.to], ...v }) + "\n");
  if (v.key === a.key && v.index === a.to && v.entryAt === a.toAt) rOk++; else rBad.push(a.key);
}
const sum = { ranges: all.length, archivedTo: all.at(-1)?.to, sampled: pick.length, verified: ok, bytes, entries, bad, restoredTo: { ok: rOk, bad: rBad } };
appendFileSync(OUT, JSON.stringify({ kind: "summary", at: new Date().toISOString(), ...sum }) + "\n");
log(`b030 verify: ${JSON.stringify(sum)}`);
