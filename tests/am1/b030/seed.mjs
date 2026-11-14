// B0-30's 4 GB metadata tier on all three members of m3org/b0-30, as BF-20 seeded it (`tests/m2/bf20/seed.py`):
// `/bench_seed {"start","n","bytes":1800,"written"}` puts comment rows of 1,800 bytes, 10,000 a call, the same keys on
// every member, until each member's SQLite (`databaseSize`, read as an f64) passes TARGET bytes. Snapshots are held
// while it runs and released at the end. Resumable: progress per site is in tests/am1/raw/b030-seed.<site>, one
// `start n ms dbBytes digest` line a call. Ends by checking that the three members' metadata digests are equal.
//   node tests/am1/b030/seed.mjs [TARGET=4000000000]
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { log, member, sleep } from "../hosted/lib.mjs";
const repo = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo;
const chain = `repo:${repo}`;
const TARGET = Number(process.argv[2] ?? 4_000_000_000);
const BATCH = 10_000;
const WRITTEN = 2;
// Per repository, so a new repository never resumes from another one's finished seed (smoke 3 on repo:1048588 read
// smoke 2's files, seeded no row, and split a 1.4 MB group).
const file = (s) => `tests/am1/raw/b030-seed.${repo}.${s}`;
const lines = (s) => (existsSync(file(s)) ? readFileSync(file(s), "utf8").trim().split("\n").filter(Boolean).map((l) => l.split(" ")) : []);

for (const s of [0, 1, 2]) log(`hold ${s}: ${JSON.stringify((await member(chain, s, "/bench_seed", { snap: "hold" })).v)}`);

async function run(s) {
  let done = lines(s);
  let start = done.reduce((m, l) => Math.max(m, +l[0] + +l[1]), 0);
  let db = done.length ? +done.at(-1)[3] : 0;
  let fails = 0;
  while (db < TARGET) {
    const t = Date.now();
    let v;
    try {
      v = (await member(chain, s, "/bench_seed", { start, n: BATCH, bytes: 1800, written: WRITTEN }, 55_000)).v;
      if (!v?.digest) throw new Error(JSON.stringify(v).slice(0, 200));
    } catch (e) {
      fails++;
      log(`site ${s} start ${start} failed (${fails}): ${String(e).slice(0, 200)}`);
      await sleep(Math.min(60_000, 5_000 * fails));
      continue;
    }
    fails = 0;
    db = v.dbBytes;
    appendFileSync(file(s), `${start} ${BATCH} ${Date.now() - t} ${db} ${v.digest}\n`);
    start += BATCH;
    if ((start / BATCH) % 10 === 0) log(`site ${s} ${start} rows, db ${(db / 1e9).toFixed(3)} GB`);
  }
  return start;
}
// The sites seed in parallel, each to TARGET; then every member is brought to the largest row count, so the rows are
// the same on all three.
const ends = await Promise.all([0, 1, 2].map(run));
const rows = Math.max(...ends);
for (const s of [0, 1, 2]) {
  let start = ends[s];
  while (start < rows) {
    const v = (await member(chain, s, "/bench_seed", { start, n: BATCH, bytes: 1800, written: WRITTEN }, 55_000)).v;
    if (!v?.digest) { await sleep(5_000); continue; }
    appendFileSync(file(s), `${start} ${BATCH} 0 ${v.dbBytes} ${v.digest}\n`);
    start += BATCH;
  }
}
// Each member's own metadata digest now: the seed reply's is at the index each member had applied when it seeded.
const folds = [];
for (const s of [0, 1, 2]) folds.push((await member(chain, s, "/fold", { tier: 1 })).v);
log(`folds ${JSON.stringify(folds).slice(0, 1500)}`);
const digests = [0, 1, 2].map((s) => lines(s).at(-1)[4]);
const db = [0, 1, 2].map((s) => +lines(s).at(-1)[3]);
for (const s of [0, 1, 2]) log(`release ${s}: ${JSON.stringify((await member(chain, s, "/bench_seed", { snap: "release" })).v)}`);
const out = { chain, rows, bytesPerRow: 1800, written: WRITTEN, dbBytes: db, digests, equal: new Set(digests).size === 1, at: new Date().toISOString() };
writeFileSync("tests/am1/raw/b030-seed.json", JSON.stringify(out) + "\n");
log(`seeded ${JSON.stringify(out)}`);
