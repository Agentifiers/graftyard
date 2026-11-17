// b030r6 close-out (operator, 2026-11-17): whether each "http 0" push landed. landedpush.mjs could not say: the
// followers had truncated their stored log to 18,640 (19:20Z) by close-out, so the entries of the earlier bursts were
// gone from /log. This reads git instead.
//   node tests/am1/b030split/landedanc.mjs [N]
// For each http-0 push: fetch its ref through the edge and check whether the pushed commit is an ancestor of the
// ref's tip now (the driver builds each later push on the tip it reads after a stale answer, so a landed push stays in
// the branch's history and an unlanded one never enters it).
import { readFileSync, appendFileSync } from "node:fs";
import { Git, tokens } from "../hosted/lib.mjs";
const OUT = "tests/am1/raw/b030r6-landed-anc.jsonl";
const rows = readFileSync("tests/am1/raw/b030r6-fetchfail.jsonl", "utf8").trim().split("\n").map(JSON.parse).filter((r) => r.commit && r.ms !== undefined);
const only = process.argv[2] ? Number(process.argv[2]) : rows.length;
const g = new Git("m3org/b0-30-r6").as(tokens.admin);
g.run(["init", "-q", "--bare", g.wt], g.dir);
let landed = 0, n = 0;
for (const r of rows.slice(0, only)) {
  const t0 = Date.now();
  const f = g.run(["fetch", "-q", g.url, `+${r.ref}:${r.ref}`]);
  const anc = f.ok ? g.run(["merge-base", "--is-ancestor", r.commit, r.ref]) : { ok: false };
  const tip = f.ok ? g.run(["rev-parse", r.ref]).out.trim() : null;
  const row = { ref: r.ref, commit: r.commit, failedAt: r.at, fetched: f.ok, tip, landed: anc.ok, ms: Date.now() - t0, err: f.ok ? undefined : f.out.slice(0, 200) };
  appendFileSync(OUT, JSON.stringify(row) + "\n");
  n++; if (anc.ok) landed++;
  console.log(r.ref, r.commit.slice(0, 8), f.ok ? (anc.ok ? "LANDED (ancestor of tip)" : "NOT an ancestor") : "fetch failed", row.ms, "ms");
}
console.log(JSON.stringify({ checked: n, landed }));
