// pool-grants' two-owner smoke on graft-m3c (`design/DESIGN.md`, Shape, `HOT_POOL`; BUILD-PLAN B0-34's hostile-owner
// lines). Owner A is org m3org with one network (`pg-mono`); owner B is user m3-admin with three (`pg-h1..3`). Each
// step asks a network's group leader for k through the test-faults `/bench_seed {"fanout": k}`, which runs the same
// grant path as a load report (`workers/site/src/fanout.rs`, `Grant::ask`): both warm sites' pool ledgers are asked,
// and k = 1 + the fewer granted is committed. The driver reads the committed k back from the fold (`ranges`) each
// time. No load and no containers, except two pushes to pg-mono that check the first shard forwards to its granted
// slots. Phases:
//   1. A steps up to 7 (6 pool shards on each warm site, its share).
//   2. B steps each of its three networks to 8: B gets only what is free; A's k stays 7.
//   3. Both ledgers of A down (`pool_down`): A asks 3 and B's h1 asks 8; neither k moves.
//   4. A steps down to 1, releasing its slots; B asks 8 on each network again: B holds at most its share, 6.
// After phases 2 and 4, no ledger leases a network more pool slots than its committed k uses (the leader gives back
// what only one warm site granted).
//   5. Expiry: B stops asking, A renews at k = 2 each minute; after LOAD_STALE, A asks 8 and gets B's freed slots.
// One JSON line per check on stdout; exit 1 on any failed check. Environment as tests/am1/hosted/lib.mjs.
//   node tests/am1/pool/smoke.mjs
import { Git, api, leaderOf, log, member, sleep, tokens, bedGet } from "../hosted/lib.mjs";

const fails = [];
const check = (name, ok, v) => { console.log(JSON.stringify({ check: name, ok, ...v })); if (!ok) fails.push(name); };
const suffix = process.env.PG_SUFFIX ?? Date.now().toString(36).slice(-5);

async function repo(name, owner) {
  const path = owner === "org" ? "/api/v3/orgs/m3org/repos" : "/api/v3/user/repos";
  let r = await api("POST", path, tokens.admin, { name, private: false, auto_init: true });
  const full = `${owner === "org" ? "m3org" : "m3-admin"}/${name}`;
  // `api` re-sends a call whose answer was lost; a create that landed then reads "name already exists".
  if (r.status === 422 && r.body?.errors?.[0]?.message === "name already exists on this account") {
    r = await api("GET", `/api/v3/repos/${full}`, tokens.admin);
    if (r.status === 200) r.status = 201;
  }
  if (r.status !== 201) throw new Error(`${name}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  // The group is created on its first read.
  await api("GET", `/api/v3/repos/${full}`, tokens.admin);
  log(`${full} = ${r.body.id} (owner ${r.body.owner?.id})`);
  return { name, full, id: r.body.id, chain: `repo:${r.body.id}`, owner: r.body.owner?.id };
}

/** Asks `n`'s leader for k (optionally with its warm ledgers down for `down` ms); answers the leader's reply and the
 * committed k read back. */
async function ask(n, k, down) {
  // An idle group has no leader; a read through the edge elects one, as `tests/fanout/k.mjs` does.
  let leader;
  for (let i = 0; leader === undefined; i++) {
    await api("GET", `/api/v3/repos/${n.full}`, tokens.admin);
    try { leader = await leaderOf(n.chain); } catch (e) { if (i >= 3) throw e; }
  }
  const body = { fanout: k, ...(down ? { pool_down: down } : {}) };
  const r = await member(n.chain, leader, "/bench_seed", body, 30_000);
  const v = await member(n.chain, leader, "/read", { q: { ranges: true } }, 20_000);
  const committed = v.v?.value?.fanout;
  const grants = Object.fromEntries((r.v?.grants ?? []).map(([s, g]) => [s, g]));
  log(`${n.name} ask ${k}${down ? ` (ledgers down ${down} ms)` : ""}: ${JSON.stringify(r.v ?? r.status).slice(0, 300)}; committed ${committed}`);
  return { k: committed, reply: r.v, grants };
}
const held = (a) => Object.values(a.grants).map((g) => (g ?? []).length);

const A = await repo(`pg-mono-${suffix}`, "org");
const B = [];
for (let i = 1; i <= 3; i++) B.push(await repo(`pg-h${i}-${suffix}`, "user"));
check("two owners", A.owner !== B[0].owner && B.every((b) => b.owner === B[0].owner), { a: A.owner, b: B[0].owner });

// 1. A steps up to 7.
const a1 = await ask(A, 7);
check("1 A steps up to 7", a1.k === 7, { k: a1.k, held: held(a1) });
const aSlots = a1.grants;

// Two pushes to pg-mono: the first one's report renews A's lease on its site's ledger and the first shard keeps the
// granted slots, which its status then shows.
if (!process.env.PG_NO_PUSH) {
  const g = new Git(A.full).as(tokens.admin);
  const c = g.clone();
  for (let i = 0; i < 2 && c.ok; i++) {
    g.run(["commit", "-q", "--allow-empty", "-m", `pg ${i}`]);
    const p = g.run(["push", "-q", "origin", "HEAD:main"]);
    log(`push ${i}: ${p.ok ? "ok" : p.out.slice(0, 200)}`);
    await sleep(15_000);
  }
  const st = [];
  for (const s of [0, 1, 2]) {
    const r = await bedGet(`/_bed/shard_status?repo=${A.id}&shard=${s}`);
    let v = null; try { v = JSON.parse(r.body); } catch { v = r.body.slice(0, 200); }
    st.push({ site: s, status: r.status, k: v?.k, pool: v?.pool });
  }
  const sites = st.filter((x) => Array.isArray(x.pool) && x.pool.length);
  check("first shard keeps the granted slots", c.ok && sites.length >= 1 &&
    sites.every((x) => JSON.stringify(x.pool) === JSON.stringify(aSlots[x.site])), { clone: c.ok, st, granted: aSlots });
}

// 2. B steps each of its networks to 8.
const b2 = [];
for (const b of B) b2.push(await ask(b, 8));
const a2 = await ask(A, 7);
check("2 A's k is not reduced by B's step up", a2.k === 7 && JSON.stringify(a2.grants) === JSON.stringify(aSlots), { k: a2.k, grants: a2.grants });
const bPool = b2.reduce((s, x) => s + (x.k - 1), 0);
check("2 B gets only the free slots", bPool <= 1, { ks: b2.map((x) => x.k) });
// After the trim, no ledger leases a network more slots than its committed k uses.
check("2 no ledger leases past k - 1", b2.every((x) => Object.values(x.grants).every((g) => (g ?? []).length <= x.k - 1)),
  { grants: b2.map((x) => [x.k, x.grants]) });
for (const [s, g] of Object.entries(aSlots)) {
  const others = b2.flatMap((x) => x.grants[s] ?? []);
  check(`2 no slot on site ${s} held twice`, others.every((x) => !g.includes(x)), { a: g, b: others });
}

// 3. A's ledgers down: no step down, and B's h1 no step up.
const a3 = await ask(A, 3, 120_000);
check("3 unreachable ledger: A keeps 7 (no step down)", a3.k === 7, { k: a3.k, grants: a3.grants });
const h1k = b2[0].k;
const b3 = await ask(B[0], 8, 120_000);
check("3 unreachable ledger: h1 keeps its k (no step up)", b3.k === h1k, { k: b3.k, was: h1k });
log("waiting 125 s for the ledgers to come back");
await sleep(125_000);

// 4. A releases; B asks again: held to its share of 6, with a pool of 7 free.
const a4 = await ask(A, 1);
check("4 A steps down to 1 and releases", a4.k === 1, { k: a4.k, grants: a4.grants });
const b4 = [];
for (const b of B) b4.push(await ask(b, 8));
const bHeld = b4.reduce((s, x) => s + (x.k - 1), 0);
check("4 no ledger leases past k - 1", b4.every((x) => Object.values(x.grants).every((g) => (g ?? []).length <= x.k - 1)),
  { grants: b4.map((x) => [x.k, x.grants]) });
check("4 B held to its share of 6 across three networks", bHeld <= 6 && bHeld >= 1, { ks: b4.map((x) => x.k), held: bHeld });
for (const s of Object.keys(b4[0].grants)) {
  const per = b4.map((x) => (x.grants[s] ?? []).length).reduce((p, q) => p + q, 0);
  check(`4 B's slots on site ${s} within its share`, per <= 6, { slots: b4.map((x) => x.grants[s]) });
}

// 5. Expiry: B stops asking; A renews at k = 2 every minute so the ledgers stay in use; after LOAD_STALE (180 s) B's
// leases are gone and A's step up to 8 takes their slots (its own share, 6, since it now has the pool to itself).
const t5 = Date.now();
await ask(A, 2);
while (Date.now() - t5 < 200_000) { await sleep(55_000); await ask(A, 2); }
const a5 = await ask(A, 8);
check("5 B's unrenewed leases expired and freed their shards", a5.k === 7, { k: a5.k, grants: a5.grants });

// Leave nothing leased: every network back to k = 1.
for (const n of [A, ...B]) await ask(n, 1);
console.log(JSON.stringify({ done: true, fails, repos: [A, ...B].map((n) => [n.full, n.id]) }));
process.exit(fails.length ? 1 : 0);
