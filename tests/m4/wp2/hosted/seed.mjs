// BF-67's stargazers on graft-m4a: N real accounts, each with its `Names` rows (Account, login, AccountLogin,
// Placement copied from the install's `Tri`), its own `Identity` group (`Created`), and a star on one repository as a
// `SetRecord` under the account's own web-session credential, in the same entry as `Created`. The `Counts` consumer on
// each account's `Identity` leader then copies the star into the repository's watcher index, which `/stargazers`
// lists. Run after a site day with `GRAFTYARD_ACTIVITY=1`. Idempotent: a re-run keeps what exists.
//
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp2/hosted/seed.mjs <repo id> <from> <to> [conc]     (accounts FIRST+from .. FIRST+to-1)
//
// Writes one JSON line per account to tests/m4/wp2/raw/seed-<from>-<to>.jsonl.
import { mkdirSync } from "node:fs";
import { OP, accountShard, log, loginShard, rid, row, sleep, tools } from "../../../am1/hosted/lib.mjs";

const [repo, from, to, conc = "48"] = process.argv.slice(2).map(Number);
if (!repo || !(to > from)) throw new Error("usage: seed.mjs <repo id> <from> <to> [conc]");
export const FIRST = 7300000;
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
const out = new URL(`../raw/seed-${from}-${to}.jsonl`, import.meta.url).pathname;
mkdirSync(new URL("../raw/", import.meta.url).pathname, { recursive: true });

const okOr = (o, ...allowed) => o?.ok || allowed.some((a) => JSON.stringify(o ?? {}).includes(a));
// A `Names` shard's group is made on first use, as the edge's `ensure_group` does; once per chain per run.
const created = new Map();
const ensure = (chain) => {
  if (!created.has(chain)) created.set(chain, tools("/create", { chain }).catch((e) => { created.delete(chain); throw e; }));
  return created.get(chain);
};
async function commit(chain, cmd, ...allowed) {
  await ensure(chain);
  for (let i = 0; ; i++) {
    let r;
    try { r = await tools("/commit", { chain, cmd, rid: rid() }); } catch (e) { r = { error: String(e) }; }
    if (r.ok && okOr(r.outcome, ...allowed)) return r.outcome;
    if (r.ok && r.outcome?.refused) throw new Error(`${chain}: ${JSON.stringify(r.outcome).slice(0, 200)}`);
    if (i >= 4) throw new Error(`${chain}: ${JSON.stringify(r).slice(0, 200)}`);
    await sleep(1000 * (i + 1));
  }
}

async function one(i) {
  const a = FIRST + i, login = `wp2-s${i}`, t0 = Date.now();
  const home = `names:${accountShard(a)}`;
  await commit(home, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await commit(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind: "User", account: a } } }, "Taken");
  await commit(home, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  await commit(home, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
  const chain = `identity:${a}`;
  await ensure(chain);
  const me = { account: a, key: { Session: { issued: t0 } } };
  const r = await tools("/commit_many", { chain, cmds: [
    { cred: OP, op: { Create: { login, kind: "User", created: t0 } } },
    { cred: me, op: { SetRecord: { Star: { repo, on: true } } } },
  ] });
  const res = r.results ?? [];
  // Each result is `{done: {ok|refused}}`, or `{admitted}` when held past COMMIT_HOLD (committed later).
  const bad = res.filter((x) => !(x?.done?.ok || x?.admitted || JSON.stringify(x?.done ?? {}).includes("AlreadyCreated")));
  return { i, a, ms: Date.now() - t0, leader: r.leader?.site, results: res.map((x) => JSON.stringify(x).slice(0, 120)), bad: bad.length };
}

let next = from, done = 0, failed = 0;
const t0 = Date.now();
await Promise.all(Array.from({ length: conc }, async () => {
  while (next < to) {
    const i = next++;
    try {
      const v = await one(i);
      if (v.bad) failed++;
      row(out, v);
    } catch (e) {
      failed++;
      row(out, { i, error: String(e).slice(0, 300) });
    }
    if (++done % 100 === 0) log(`${done}/${to - from} in ${Math.round((Date.now() - t0) / 1000)} s, ${failed} failed`);
  }
}));
log(`done ${done}, failed ${failed}, ${Math.round((Date.now() - t0) / 1000)} s`);
