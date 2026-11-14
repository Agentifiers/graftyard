// Re-homes a repository group on site <to> and moves its leader there, for B0-31's apac leader. The member route
// `/move` on the leader runs `graftyard_identity::group_move::next_step` against a record with the same sites and
// `home` <to>: `Home` commits a `Normal` config whose placement's home is <to>, then `Transfer` moves leadership, then
// `Done`. The raft core's `maybe_hand_off` returns leadership to the committed config's home, so after this the apac
// leader is the stable state rather than one the core undoes within a window (rows-m19, B0-31).
// Prints the members' roles and the config's home before and after.
//   node tests/am1/runs/transfer.mjs <chain> <to> [repo name, read first to wake the group]
import { api, leaderOf, member, sleep, statuses, tokens } from "../hosted/lib.mjs";
const [chain, to] = [process.argv[2], Number(process.argv[3])];
const home = (s) => s.config?.config?.normal?.home ?? s.config?.config?.joint?.new?.home ?? null;
const roles = async () => (await statuses(chain)).map((s) => [s.site, s.role, s.term, home(s)]);
// An idle group may have no leader (campaigns are request-driven): one repository read through the edge wakes it.
if (process.argv[4]) await api("GET", `/api/v3/repos/m3org/${process.argv[4]}`, tokens.admin);
console.log("before", JSON.stringify(await roles()));
// `next_step` reads only the target's sites and home; the other fields are the record's shape.
const target = { name: "Tri", sites: [0, 1, 2], home: to, generation: 1, moving_from: null, at: 0 };
let done = false;
for (let i = 0; i < 12 && !done; i++) {
  const l = await leaderOf(chain);
  const r = await member(chain, l, "/move", { to: target }, 60_000);
  console.log("move on", l, JSON.stringify(r.v ?? r.status).slice(0, 240));
  done = r.v?.step === "Done";
  if (!done) await sleep(3000);
}
// Held: the leader is still on <to> after the hand-off window would have returned it. An idle group may sit with
// no leader (F67: only a real request wakes it), so one repository read wakes it before the roles are read.
await sleep(15_000);
if (process.argv[4]) await api("GET", `/api/v3/repos/m3org/${process.argv[4]}`, tokens.admin);
await sleep(3_000);
const after = await roles();
console.log("after", JSON.stringify(after));
const lead = after.filter((s) => s[1] === "leader").sort((a, b) => b[2] - a[2])[0];
console.log(done && lead?.[0] === to && lead?.[3] === to ? `rehomed on ${to}` : `NOT rehomed on ${to}`);
