// pool-lease-expiry's hosted check on graft-m3c: a lease nobody renews lapses through the ledger object's heartbeat
// alone. One m3org network asks k = 4 through its leader (`/bench_seed {"fanout": 4}`, the grant path), then nothing
// calls either warm site's ledger again. The driver reads each ledger as stored (`/_bed/pool`, which runs nothing on
// the ledger) every 30 s for 8 minutes: the lease must be gone by LOAD_STALE (180 s) plus two beats (2 x 45 s)
// after the ask, and the ledger's `seen` must keep moving while a lease is held. On the code before the heartbeat,
// the lease stays until the next call. Prints one JSON line per read and a verdict; exit 1 on a failed check.
// Environment as tests/am1/hosted/lib.mjs.
//   node tests/am1/pool/expiry.mjs
import { api, bedGet, leaderOf, log, member, sleep, tokens } from "../hosted/lib.mjs";

const name = `pg-exp-${Date.now().toString(36).slice(-5)}`;
let r = await api("POST", "/api/v3/orgs/m3org/repos", tokens.admin, { name, private: false, auto_init: true });
if (r.status === 422 && r.body?.errors?.[0]?.message === "name already exists on this account") {
  r = await api("GET", `/api/v3/repos/m3org/${name}`, tokens.admin);
  if (r.status === 200) r.status = 201;
}
if (r.status !== 201) throw new Error(`${name}: ${r.status}`);
const id = r.body.id, chain = `repo:${id}`;
let leader;
for (let i = 0; leader === undefined; i++) {
  await api("GET", `/api/v3/repos/m3org/${name}`, tokens.admin);
  try { leader = await leaderOf(chain); } catch (e) { if (i >= 3) throw e; }
}
const ask = await member(chain, leader, "/bench_seed", { fanout: 4 }, 30_000);
const t0 = Date.now();
log(`${name} = ${id}: ask 4: ${JSON.stringify(ask.v).slice(0, 300)}`);
const sites = (ask.v?.grants ?? []).filter(([, g]) => (g ?? []).length).map(([s]) => s);
const fails = [];
if (ask.v?.k !== 4 || sites.length !== 2) fails.push("granted k 4 on both warm sites");

const reads = [];
while (Date.now() - t0 < 480_000) {
  await sleep(30_000);
  const row = { s: Math.round((Date.now() - t0) / 1000) };
  for (const s of sites) {
    const p = await bedGet(`/_bed/pool?site=${s}`);
    let v = null; try { v = JSON.parse(p.body); } catch { v = { raw: p.body.slice(0, 120), status: p.status }; }
    const mine = (v.leases ?? []).find((l) => l.network === id);
    row[s] = { held: mine?.slots?.length ?? 0, seen_s: v.seen ? Math.round((v.seen - t0) / 1000) : null, leases: (v.leases ?? []).length };
  }
  console.log(JSON.stringify(row));
  reads.push(row);
}
// Gone by LOAD_STALE plus two beats after the ask, on both sites, with no call to either ledger.
const bound = 180 + 90 + 30;
for (const s of sites) {
  const after = reads.filter((x) => x.s >= bound);
  if (!after.length || after.some((x) => x[s].held)) fails.push(`site ${s}: lease still held past ${bound} s`);
  const beats = new Set(reads.filter((x) => x[s].held).map((x) => x[s].seen_s));
  if (reads.filter((x) => x[s].held).length >= 3 && beats.size < 2) fails.push(`site ${s}: seen never moved while a lease was held`);
  const first = reads.find((x) => !x[s].held);
  log(`site ${s}: lease gone at the read at ${first?.s ?? "never"} s; seen values while held ${[...beats].join(",")}`);
}
console.log(JSON.stringify({ done: true, repo: [name, id], fails }));
process.exit(fails.length ? 1 : 0);
