// BF-53's leader-read check: for each chain in a file (one per line), every member's role and its `/stats` inbound
// `/log_rows` count, through the tools edge (`lib.mjs`, each site's current voter). `snap` saves the counts and who
// leads; `read` reports each count's rise since the snap (a chain not snapped counts from 0), summed on members that
// lead now or led at the snap, and on the others. The edge's entries endpoint reads a learner or follower, never the
// leader (`graftyard_lookup::log_rows`), so the leaders' sum must be 0.
//   node tests/m6/hosted/bf53-leaders.mjs <chains file> snap|read <snap.json>
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { member, statuses } from "../../am1/hosted/lib.mjs";

const [FILE, MODE, SNAP] = process.argv.slice(2);
const chains = readFileSync(FILE, "utf8").split("\n").filter(Boolean);
const one = async (chain) => {
  const st = await statuses(chain).catch(() => []);
  return {
    chain,
    members: await Promise.all(st.map(async (m) => ({
      site: m.site,
      role: m.role ?? (m.joined === false ? "not joined" : m.error ? "error" : null),
      logRows: await member(chain, m.site, "/stats", undefined, 15_000, m.doName).then((r) => r.v?.inbound?.["/log_rows"] ?? 0, () => null),
    }))),
  };
};
const res = [];
for (let i = 0; i < chains.length; i += 8) res.push(...await Promise.all(chains.slice(i, i + 8).map(one)));
if (MODE === "snap") {
  writeFileSync(SNAP, JSON.stringify(Object.fromEntries(res.map((r) => [r.chain, r.members]))));
  console.log(JSON.stringify({ snapped: res.length, members: res.map((r) => [r.chain, r.members.map((m) => `${m.site}:${m.role}:${m.logRows}`)]) }));
} else {
  const snap = existsSync(SNAP) ? JSON.parse(readFileSync(SNAP, "utf8")) : {};
  let leader = 0, follower = 0, unread = 0;
  const rows = [];
  for (const r of res) {
    for (const m of r.members) {
      const was = (snap[r.chain] ?? []).find((x) => x.site === m.site);
      if (m.logRows === null) { unread++; continue; }
      const rise = m.logRows - (was?.logRows ?? 0);
      const led = m.role === "leader" || was?.role === "leader";
      if (led) leader += rise; else follower += rise;
      if (rise) rows.push(`${r.chain}@${m.site} ${led ? "leader" : m.role} +${rise}`);
    }
  }
  console.log(JSON.stringify({ chains: res.length, log_rows_on_leaders: leader, log_rows_on_followers: follower, unread }));
  for (const x of rows.slice(0, 40)) console.log(x);
}
