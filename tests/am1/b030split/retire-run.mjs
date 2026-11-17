// A B0-30 short run's teardown (b030r6, operator 2026-11-17): retire every joined member of the run's repository
// chain and its metadata child, found by a scan of generations 1 to GENS on all three sites rather than a list, since a
// re-placement leaves its removed member joined (b030r5: the parent's and the child's `#1#f1`). For each joined member
// the last `/status` and size (`/bench_seed {b030:{size:true}}`) go to tests/am1/raw/$B030_RUN-final.jsonl, then the
// test route `/retire` (BF-11's lost-member stub: storage deleted, object reset). The scan runs again; once it finds no
// joined member, tests/fanout/purge.py deletes the network's bucket objects (R25) from graft-m1's buckets after its
// own check of the voters. NO_PURGE=1 skips that.
//   B030_RUN=b030r6 node tests/am1/b030split/retire-run.mjs
import { appendFileSync, readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { member } from "../hosted/lib.mjs";

const RUN = process.env.B030_RUN ?? "b030r6";
const GENS = Number(process.env.GENS ?? 10);
const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo;
const LOG = `tests/am1/raw/b030-retire-${RUN}.log`, FINAL = `tests/am1/raw/${RUN}-final.jsonl`;
const log = (s) => { const l = `${new Date().toISOString()} ${s}`; console.log(l); appendFileSync(LOG, l + "\n"); };
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
const brief = (v) => v?.joined === false ? "not joined" : v?.error ? `error ${v.error}` : `${v?.role} t${v?.term} c${v?.commit}`;
const scan = async () => {
  const live = [];
  for (const c of [`repo:${REPO}`, `repo-meta:${REPO}`]) for (let g = 1; g <= GENS; g++) for (const s of [0, 1, 2]) {
    const n = `${c}#${g}#f${s}`, st = await status(c, s, n);
    if (st?.joined !== false) live.push([c, s, n, st]);
  }
  return live;
};
for (const [c, s, n, st] of await scan()) {
  const size = st?.error ? null : await member(c, s, "/bench_seed", { b030: { size: true } }, 30_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
  if (size) delete size.meta;
  appendFileSync(FINAL, JSON.stringify({ at: new Date().toISOString(), chain: c, site: s, name: n, status: st, size }) + "\n");
  const r = await member(c, s, "/retire", {}, 60_000, n).catch((e) => ({ status: String(e).slice(0, 80) }));
  log(`${n} before: ${brief(st)}; /retire: ${JSON.stringify(r?.v ?? r?.status ?? null).slice(0, 80)}`);
}
await new Promise((r) => setTimeout(r, 15_000));
const left = await scan();
for (const [, , n, st] of left) log(`${n} after: ${brief(st)}`);
log(`scan of generations 1 to ${GENS}, both chains, three sites: ${left.length} joined`);
if (!left.length && !process.env.NO_PURGE) {
  const out = execFileSync("uv", ["run", "-q", "--with", "boto3", "python3", "tests/fanout/purge.py", String(REPO)], {
    encoding: "utf8", env: { GRAFT_TOOLS_EDGE: "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev", ...process.env, INSTALL: "graft-m1", BUCKET: "{install}-packs-{site}", PURGE_LOG: `tests/am1/raw/${RUN}-purge.log` } });
  for (const l of out.trim().split("\n")) log(`purge: ${l.slice(21)}`);
}
