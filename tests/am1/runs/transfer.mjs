// Moves a repository group's leader to site <to> (member `/transfer` on the current leader, as lf13.mjs does), for
// B0-31's apac leader. Prints the members' roles before and after.
//   node tests/am1/runs/transfer.mjs <chain> <to> [repo name, read first to wake the group]
import { api, leaderOf, member, sleep, statuses, tokens } from "../hosted/lib.mjs";
const [chain, to] = [process.argv[2], Number(process.argv[3])];
const roles = async () => (await statuses(chain)).map((s) => [s.site, s.role, s.term]);
// An idle group may have no leader (campaigns are request-driven): one repository read through the edge wakes it.
if (process.argv[4]) await api("GET", `/api/v3/repos/m3org/${process.argv[4]}`, tokens.admin);
console.log("before", JSON.stringify(await roles()));
for (let i = 0; i < 5; i++) {
  const l = await leaderOf(chain);
  if (l === to) break;
  const t = await member(chain, l, "/transfer", { to }, 15_000);
  console.log("transfer from", l, "->", to, JSON.stringify(t.v ?? t.status).slice(0, 200));
  await sleep(3000);
}
console.log("after", JSON.stringify(await roles()));
