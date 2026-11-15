// The leader's admission of one repository group, every second, for LM-1's B0-31 trace on graft-m3f: whether the
// commands a row sends shrink `Admission::capacity` (ENTRY_SOFT / `cmd_bytes` commands per entry at the measured
// round) until agent commands wait out AGENT_WAIT. Each line: the members' roles and terms, and the leader's
// `/status` `admission` (capacity, roundMs, cmdBytes, queued, agentQueued, agentBytes, hot, pressure).
// Waits for the repository's id to appear in am1-runs-repos.txt (rows.sh creates it).
//   node tests/fanout/admit.mjs <repo name> <seconds>   -> stdout, one JSON line a second
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
while (Date.now() < until) {
  const t = Date.now();
  const st = await statuses(chain);
  const lead = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  console.log(JSON.stringify({ at: new Date(t).toISOString(), ms: Date.now() - t, members: st.map((s) => [s.site, s.role ?? "?", s.term ?? null]),
    leader: lead?.site ?? null, admission: lead?.admission ?? null }));
  await sleep(Math.max(0, 1_000 - (Date.now() - t)));
}
