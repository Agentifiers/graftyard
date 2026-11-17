// learners.mjs CHAIN LOAD : places learners on a repository's or an owner's `Authz` group through its leader's `/learners` route
// (`place_learners` from a `GroupLoad`). A new learner names no leader, and refuses reads as `notLeader`, until an
// append reaches it, so rows.sh runs this before k.mjs, whose commit is that append.
//   node tests/fanout/learners.mjs repo:ID enam:1500[,weur:600]
// LOAD is the group's read rate per region, journey 9's whole-tier reads weighted by 50 (design/DESIGN.md, Scale, learners).
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { leaderOf, log, member } from "../am1/hosted/lib.mjs";

const [chain, load] = process.argv.slice(2);
if (!/^(repo|authz):/.test(chain ?? "") || !load) throw new Error("usage: learners.mjs repo:ID|authz:ID region:rate[,region:rate]");
const reads = load.split(",").map((x) => { const [r, n] = x.split(":"); return [r, Number(n)]; });
// A fresh group's config is still joint while its background placement runs (fresh-repo, dd046f2): `/learners` answers
// "the committed config is not Normal" until it settles, so wait up to 120 s for it (rows m24s33 placed none). Any
// other error is asked again within the same 120 s: on m3fr12's B0-32 the create's background re-placement of f1
// committed (16:05:37.9Z) while `/learners` was joining its 8 learners (16:05:32.7 to 39.6Z), so its change from the
// config it read first committed nothing, and the row ran on its two followers alone. The error text was not logged.
let p, v;
for (const t0 = Date.now(); ; ) {
  const leader = await leaderOf(chain);
  p = await member(chain, leader, "/learners", { load: { reads } }, 120_000);
  v = p.v ?? {};
  if ((!v.error && v.configIndex !== undefined) || Date.now() - t0 > 120_000) break;
  await new Promise((r) => setTimeout(r, 3_000));
}
const leader = await leaderOf(chain);
// The outcome first, so a cut line still shows it.
log(`${chain} leader ${leader}: ${v.error ? `LEARNERS FAILED ${JSON.stringify(v.error)}` : `config ${v.configIndex}`} learners ${JSON.stringify(v.learners ?? p.status).slice(0, 400)}`);
process.exit(v.error || v.configIndex === undefined ? 1 : 0);
