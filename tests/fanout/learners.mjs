// learners.mjs CHAIN LOAD : places learners on a repository's or an owner's `Authz` group through its leader's `/learners` route
// (`place_learners` from a `GroupLoad`). A new learner names no leader, and refuses reads as `notLeader`, until an
// append reaches it, so rows.sh runs this before k.mjs, whose commit is that append.
//   node tests/fanout/learners.mjs repo:ID enam:1500[,weur:600]
// LOAD is the group's read rate per region, journey 9's reads weighted by 6 (design/DESIGN.md, Scale, learners).
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { leaderOf, log, member } from "../am1/hosted/lib.mjs";

const [chain, load] = process.argv.slice(2);
if (!/^(repo|authz):/.test(chain ?? "") || !load) throw new Error("usage: learners.mjs repo:ID|authz:ID region:rate[,region:rate]");
const reads = load.split(",").map((x) => { const [r, n] = x.split(":"); return [r, Number(n)]; });
const leader = await leaderOf(chain);
const p = await member(chain, leader, "/learners", { load: { reads } }, 120_000);
const v = p.v ?? {};
log(`${chain} leader ${leader}: learners ${JSON.stringify(v.learners ?? v.error ?? p.status).slice(0, 400)} config ${v.configIndex}`);
process.exit(v.error ? 1 : 0);
