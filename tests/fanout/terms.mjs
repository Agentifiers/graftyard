// Leader timeline of one repository group on graft-m2b, for LM-1's churn trace (round 12): every second, each member's
// role, term, leader, vote and promise (`promiseUntil` less the member's `now`, in ms: a follower withholding its vote
// while its promise to the leader stands) from `/status`, printed only when one of them changes (the promise only
// when it starts or lapses), until `seconds` pass. Waits for the repository's id to appear in am1-runs-repos.txt
// (rows.sh creates it).
//   node tests/fanout/terms.mjs <repo name> <seconds>   -> stdout, one JSON line per change
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { readFileSync } from "node:fs";
import { member, sleep, voterNames } from "../am1/hosted/lib.mjs";
import { serverClock } from "./srvclock.mjs";

const [name, secs] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
// A fresh install's state has no am1-runs-repos.txt until rows.sh creates the first repository, so a missing file
// waits like a missing line (m3fhg: terms.mjs started 58 s before the file and exited ENOENT).
const idOf = () => {
  try {
    return readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
  } catch (e) {
    if (e.code === "ENOENT") return undefined;
    throw e;
  }
};
let id;
// A `repo:N` argument names the chain directly (B0-34's b034-mono is not in am1-runs-repos.txt).
if (name.startsWith("repo:")) id = name.slice(5);
while (!id && !(id = idOf()) && Date.now() < until) await sleep(2_000);
if (!id) throw new Error(`${name}: no id`);
const chain = `repo:${id}`;
// `at` is on Cloudflare's clock, the one the members' trace rows use (README, "Clocks"); the first line logs the
// orb's measured offset.
const clk = serverClock();
await clk.ready;
console.log(JSON.stringify({ at: clk.iso(), chain, clock: clk.offset() }));
let last = "";
let names = {};
while (Date.now() < until) {
  const t = Date.now(), at = clk.now();
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
  // The seventh field is the member's committed config index and its learner count, `index/learners`, so a config
  // change during a row shows as a line (learners-kept: no config may commit after `/learners` on B0-32).
  const cfg = (s) => (s.config ? `${s.config.index}/${Object.values(s.config.config ?? {})[0]?.learners?.length ?? Object.values(s.config.config ?? {})[0]?.placement?.learners?.length ?? "?"}` : null);
  const now = st.map((s) => [s.site, s.role ?? "?", s.term ?? null, s.leader ?? null, s.votedFor ?? null, promise(s), cfg(s)]);
  // The promise's ms change every sample: the key holds only whether one stands.
  const key = JSON.stringify(now.map((m) => [...m.slice(0, 5), (m[5] ?? 0) > 0, m[6]]));
  if (key !== last) console.log(JSON.stringify({ at: new Date(at).toISOString(), ms: Date.now() - t, members: now }));
  last = key;
  await sleep(Math.max(0, 1_000 - (Date.now() - t)));
}
