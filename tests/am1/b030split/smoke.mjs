// b030-split's split smoke on graft-m1: before the 30-day rerun, a fresh seeded group (tests/am1/b030/seed.mjs and
// mkrepo.mjs, as B0-30 makes it) is split for real, and the reads are checked against the text before the split.
//
// On B0-30's first group, repo:1048586, after the site day that ships the split.
// 1. Before: each member's `/bench_seed {"b030": {"size": true}}` (dbBytes, split state), and the comment rows of a
//    sample of seeded keys read by key on the leader, with their bodies' SHA-256.
// 2. The split runs on its own once the leader's member SQLite passes SPLIT_AT (7 GiB): the first run's group stands
//    at about 8.5 GB, so it starts on the leader's first alarm after the site day.
// 3. It polls until the parent's slim pass is done, logging each step, then reads the same sample again, checks each
//    body is the same and that the child holds it, and reads every member's dbBytes after.
// Rows go to tests/am1/raw/b030split-smoke.jsonl.
//   node tests/am1/b030split/smoke.mjs
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { leaderOf, log, member, row, sleep } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const C = `repo:${REPO.repo}`;
const CHILD = `repo-meta:${REPO.repo}`;
const OUT = "tests/am1/raw/b030split-smoke.jsonl";
const SAMPLE = Number(process.env.SAMPLE ?? 200);
const sha = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);

async function sizes(chain = C) {
  const out = [];
  for (const s of [0, 1, 2]) {
    const v = (await member(chain, s, "/bench_seed", { b030: { size: true } }, 60_000).catch((e) => ({ v: { error: String(e).slice(0, 120) } }))).v;
    out.push({ site: s, dbBytes: v?.dbBytes, split: v?.split, role: v?.role, applied: v?.applied, error: v?.error });
  }
  return out;
}

// Comment rows by key on the leader, through the same fill a reply's render uses (`{"rows": {"keys"}}`, test installs).
async function bodies(keys) {
  const s = await leaderOf(C);
  const q = { q: { rows: { keys: keys.map((k) => ["comment", [{ Int: k }]]) } } };
  const v = (await member(C, s, "/read", q, 300_000)).v;
  if (!Array.isArray(v.value)) throw new Error(`keys read: ${JSON.stringify(v).slice(0, 300)}`);
  const out = {};
  for (const [, key, cols] of v.value) if (cols) out[key[0].Int] = Buffer.from(cols[0].Blob).toString("latin1");
  return { index: v.index, out };
}

const before = await sizes();
log(`before ${JSON.stringify(before)}`);
row(OUT, { kind: "before", at: new Date().toISOString(), members: before });
// A sample of seeded comment keys (the B0-30 seed's keys are k + 2^40).
const keys = Array.from({ length: SAMPLE }, (_, i) => 2 ** 40 + Math.floor((i * 1_340_000) / SAMPLE));
const b0 = await bodies(keys);
log(`read before: ${Object.keys(b0.out).length} of ${SAMPLE} rows at ${b0.index}`);
row(OUT, { kind: "read-before", index: b0.index, found: Object.keys(b0.out).length, sums: Object.fromEntries(Object.entries(b0.out).map(([k, v]) => [k, sha(v)])) });

// The split runs on the leader's alarm; poll its state.
const t0 = Date.now();
let last = "";
for (;;) {
  const m = await sizes();
  const l = m.find((x) => x.role === "Leader") ?? m[0];
  const st = JSON.stringify(l.split);
  if (st !== last) { log(`split ${st} dbBytes ${l.dbBytes}`); row(OUT, { kind: "step", at: new Date().toISOString(), members: m }); last = st; }
  if (l.split?.slim?.done) break;
  if (Date.now() - t0 > 6 * 3600_000) { log("no end in 6 h"); break; }
  await sleep(60_000);
}
const child = await sizes(CHILD);
const b1 = await bodies(keys);
let same = 0;
const diff = [];
for (const [k, v] of Object.entries(b0.out)) if (b1.out[k] === v) same++; else diff.push(k);
const after = await sizes();
const sum = { at: new Date().toISOString(), splitMin: Math.round((Date.now() - t0) / 60_000), same, of: Object.keys(b0.out).length, diff: diff.slice(0, 10), before: before.map((m) => m.dbBytes), after: after.map((m) => m.dbBytes), child: child.map((m) => ({ site: m.site, dbBytes: m.dbBytes, loaded: m.split?.childLoaded })) };
row(OUT, { kind: "summary", ...sum });
log(`smoke ${JSON.stringify(sum)}`);
