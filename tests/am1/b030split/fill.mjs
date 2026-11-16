// B0-30 rerun (operator, 2026-11-16): the fill path on seeded rows the slim already dropped from the parent. Reads
// sample comment keys below the slim cursor through the parent's leader and one follower (read-index), each twice,
// timing each read, and compares each body with the pre-split baseline (tests/am1/raw/b030r2-bodies.json). The
// child's members' role, term and leader are read before and after, to show whether the fill campaigned in a
// leaderless child. Rows go to tests/am1/raw/b030r2-fill.jsonl. Read-only.
//   node tests/am1/b030split/fill.mjs [N=10]
import { appendFileSync, readFileSync } from "node:fs";
import { statuses, voter } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo;
const RUN = process.env.B030_RUN ?? "b030r2";
const C = `repo:${REPO}`, CHILD = `repo-meta:${REPO}`, OUT = `tests/am1/raw/${RUN}-fill.jsonl`;
const N = Number(process.argv[2] ?? 10);
const base = JSON.parse(readFileSync(`tests/am1/raw/${RUN}-bodies.json`, "utf8"));
const row = (r) => { appendFileSync(OUT, JSON.stringify(r) + "\n"); console.log(JSON.stringify(r).slice(0, 400)); };
const child = async (when) => row({ kind: "child", when, at: new Date().toISOString(),
  members: (await statuses(CHILD)).map((s) => ({ site: s.site, role: s.role, term: s.term, leader: s.leader, commit: s.commit, error: s.error })) });

const sts = await statuses(C);
const led = sts.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
if (!led) throw new Error("no parent leader");
const fol = sts.find((s) => s.role === "follower" && s.site !== led.site);
const cur = (await voter(C, led.site, "/bench_seed", { b030: { size: true } }, 60_000)).v.split?.slim;
if (!cur || cur.done || cur.table !== 1) throw new Error(`slim not in comment: ${JSON.stringify(cur)}`);
const below = Object.keys(base).map(Number).filter((k) => k < cur.last[0].Int).sort((a, b) => a - b).slice(0, N);
const above = Object.keys(base).map(Number).filter((k) => k > cur.last[0].Int).sort((a, b) => a - b).slice(0, 2);
row({ kind: "plan", at: new Date().toISOString(), leader: led.site, follower: fol?.site, cursor: cur.last[0].Int, slimmed: below.length, control: above.length });
await child("before");
async function read(site, keys, index, tag) {
  const q = { q: { rows: { keys: keys.map((k) => ["comment", [{ Int: k }]]) } }, ...(index ? { index: true } : {}) };
  const t0 = Date.now();
  const v = (await voter(C, site, "/read", q, 120_000).catch((e) => ({ v: { error: String(e).slice(0, 200) } }))).v;
  const ms = Date.now() - t0;
  const got = {};
  if (Array.isArray(v?.value)) for (const [, key, cols] of v.value) if (cols) got[key[0].Int] = Buffer.from(cols[0].Blob).toString("latin1");
  const same = keys.filter((k) => got[k] === base[k]).length;
  row({ kind: "read", tag, at: new Date(t0).toISOString(), site, index, keys: keys.length, same, ms,
    error: v?.error ?? v?.overloaded ?? (Array.isArray(v?.value) ? undefined : JSON.stringify(v).slice(0, 200)) });
}
for (const k of below) {
  await read(led.site, [k], false, "slimmed-leader-1st");
  await read(led.site, [k], false, "slimmed-leader-2nd");
}
if (fol) for (const k of below.slice(0, 3)) await read(fol.site, [k], true, "slimmed-follower");
for (const k of above) await read(led.site, [k], false, "unslimmed-leader");
await child("after");
