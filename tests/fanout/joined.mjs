// Close-out check: which repository chains in the given ranges still have a joined member, from each site's voter
// `/status` (`statuses`). A retired chain answers "not joined" on every site; its registry rows stay, so the fold
// audit's `homed` count does not fall after a retire, and the joined count here is the check. Exits 1 when any chain
// is still joined, so a row's close-out fails loudly.
//   node tests/fanout/joined.mjs FIRST-LAST|N ...      (source the install's env file first)
import { statuses } from "../am1/hosted/lib.mjs";

const nets = process.argv.slice(2).flatMap((a) => {
  const [x, y] = a.split("-").map(Number);
  return y ? Array.from({ length: y - x + 1 }, (_, i) => x + i) : [x];
});
const joined = [];
for (const n of nets) {
  const st = await statuses(`repo:${n}`).catch((e) => [{ error: String(e).slice(0, 60) }]);
  const roles = st.map((s) => (s.joined === false ? "-" : s.error ? "E" : (s.role ?? "?")[0])).join("");
  const learners = st.find((s) => s.config?.config?.normal)?.config?.config?.normal?.learners?.length ?? 0;
  console.log(`repo:${n} ${roles}${learners ? ` learners ${learners}` : ""}`);
  if (roles !== "---") joined.push(n);
}
console.log(joined.length ? `STILL JOINED ${joined.length}: ${joined.join(" ")}` : `none joined of ${nets.length}`);
process.exit(joined.length ? 1 : 0);
