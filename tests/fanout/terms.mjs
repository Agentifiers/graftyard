// Leader timeline of one repository group on graft-m2b, for LM-1's churn trace (round 12): every second, each member's
// role, term, leader, vote and promise (`promiseUntil` less the member's `now`, in ms: a follower withholding its vote
// while its promise to the leader stands) from `/status`, printed only when one of them changes (the promise only
// when it starts or lapses), until `seconds` pass. Waits for the repository's id to appear in am1-runs-repos.txt
// (rows.sh creates it).
//   node tests/fanout/terms.mjs <repo name> <seconds>   -> stdout, one JSON line per change
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { readFileSync } from "node:fs";
import { member, sleep, voterNames } from "../am1/hosted/lib.mjs";

const [name, secs] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
if (!id) throw new Error(`${name}: no id`);
const chain = `repo:${id}`;
console.log(JSON.stringify({ at: new Date().toISOString(), chain }));
let last = "";
let names = {};
while (Date.now() < until) {
  const t = Date.now();
  // Each site's current voter by name: a call by site alone reaches the first generation's object, and fresh-repo's
  // background placement replaces a voter (m25cap500's site 1 read its removed `#1#f1`, stale at commit 1).
  // While no member leads, `voterNames` finds no config and answers `{}`: keep the last names, or the sample reads
  // the dead first-generation object (m26c's site 1 at 07:01:04 to 07:01:18 read `#1#f1`, frozen at term 2).
  const got = await voterNames(chain).catch(() => ({}));
  if (Object.keys(got).length) names = got;
  const st = await Promise.all([0, 1, 2].map(async (s) => {
    try { return { site: s, ...(await member(chain, s, "/status", undefined, 15_000, names[s])).v }; } catch (e) { return { site: s, error: String(e).slice(0, 100) }; }
  }));
  const promise = (s) => (s.promiseUntil && s.now ? s.promiseUntil - s.now : null);
  const now = st.map((s) => [s.site, s.role ?? "?", s.term ?? null, s.leader ?? null, s.votedFor ?? null, promise(s)]);
  // The promise's ms change every sample: the key holds only whether one stands.
  const key = JSON.stringify(now.map((m) => [...m.slice(0, 5), (m[5] ?? 0) > 0]));
  if (key !== last) console.log(JSON.stringify({ at: new Date(t).toISOString(), ms: Date.now() - t, members: now }));
  last = key;
  await sleep(Math.max(0, 1_000 - (Date.now() - t)));
}
