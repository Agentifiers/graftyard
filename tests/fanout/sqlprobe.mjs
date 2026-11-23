// Hosted SQLite cost per statement call against per row, and per transaction against per byte. Every arm is its own
// member request so Workers Logs reports its CPU separately. Redirect stdout to JSONL for tests/fanout/sqlprobe.py.
//   node tests/fanout/sqlprobe.mjs repo:ID [site] [reps]
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { leaderOf, log, member, scan, sleep } from "../am1/hosted/lib.mjs";

const [chain, siteArg, repsArg] = process.argv.slice(2);
if (!chain?.startsWith("repo:")) throw new Error("usage: sqlprobe.mjs repo:ID [site] [reps]");
const reps = repsArg === undefined ? 3 : Number(repsArg);
if (!Number.isInteger(reps) || reps < 1) throw new Error("reps must be a positive integer");
const site = siteArg === undefined || siteArg === "" ? await leaderOf(chain) : Number(siteArg);
log(`${chain} sqlprobe site ${site} reps ${reps}`);

for (let rep = 1; rep <= reps; rep++) {
  const asks = [
    { arm: "a", k: 60, n: 50 },
    { arm: "b", k: 60, n: 50 },
    { arm: "a", k: 8, n: 50 },
    { arm: "b", k: 8, n: 50 },
    ...[4, 32, 128, 256].map((kb) => ({ arm: "c", kb, n: 50 })),
  ];
  for (let i = 0; i < asks.length; i++) {
    const ask = asks[i];
    const t0 = Date.now();
    const r = await member(chain, site, "/bench_seed", { sqlProbe: ask }, 60_000);
    console.log(scan(JSON.stringify({
      at: new Date(t0).toISOString(), end: new Date().toISOString(), site, rep, ask: { ...ask }, answer: r.v ?? r.status,
    })));
    if (rep < reps || i + 1 < asks.length) await sleep(2000);
  }
}
