// Leader timeline of one repository group on graft-m2b, for LM-1's churn trace (round 12): every second, each member's
// role, term, leader and vote from `/status`, printed only when one changes, until `seconds` pass. Waits for the
// repository's id to appear in am1-runs-repos.txt (rows.sh creates it).
//   node tests/fanout/terms.mjs <repo name> <seconds>   -> stdout, one JSON line per change
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { readFileSync } from "node:fs";
import { sleep, statuses } from "../am1/hosted/lib.mjs";

const [name, secs] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
if (!id) throw new Error(`${name}: no id`);
const chain = `repo:${id}`;
console.log(JSON.stringify({ at: new Date().toISOString(), chain }));
let last = "";
while (Date.now() < until) {
  const t = Date.now();
  const st = await statuses(chain);
  const now = st.map((s) => [s.site, s.role ?? "?", s.term ?? null, s.leader ?? null, s.votedFor ?? null]);
  const key = JSON.stringify(now);
  if (key !== last) console.log(JSON.stringify({ at: new Date(t).toISOString(), ms: Date.now() - t, members: now }));
  last = key;
  await sleep(Math.max(0, 1_000 - (Date.now() - t)));
}
