// shardfaultbed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with one driver route in
// front. `/drive` puts BM-1's site fault plan on every site's `Registry` object `faults`, as the bench's `/fault` does
// on a hosted install, and reads a `GitShard`'s `/status` on site 0 and site 1 under each plan:
//   whole   `{site: 0}`: site 0's shard must answer 503, site 1's 200 (BM-6: "one site's member and container classes
//           fail"; the edge's shard reads go on to another site)
//   group   `{site: 0, group_only: true}`: only Raft calls fail, so site 0's shard answers 200
//   chains  `{site: 0, chains: ["release"]}`: only that chain's members fail, so site 0's shard answers 200
//   off     no plan: 200
// A member's `/ping` on site 0 is read beside each, as the control that the plan reached the site. Each isolate rereads
// its plan at most once a second, in the background, so each arm polls for up to POLL_MS. Every other request goes to
// the site script unchanged.
import site, * as classes from "./out/index.js";

export const { Member, Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const POLL_MS = 8_000;
const SITES = [0, 1, 2];

async function plan(env, p) {
  const now = Date.now();
  const body = p ? { site: 0, from: now, until: now + 3_600_000, ...p } : { site: null, from: 0, until: 0 };
  for (const s of SITES) {
    const r = await env[`REGISTRY_${s}`].getByName("faults").fetch("https://registry/site_fault",
      { method: "PUT", body: JSON.stringify(body) });
    if (r.status !== 200) throw new Error(`site ${s} plan: ${r.status}`);
  }
}

const shard = async (env, s) => (await env[`SHARD_${s}`].getByName(`git#${s}#net:900401`).fetch("https://shard/status")).status;
const ping = async (env, s) => (await env[`MEMBER_${s}`].getByName("probe").fetch("https://member/ping")).status;

/** Polls until site 0's shard and member answer `want` (`[shard, member]`), or POLL_MS passes; the last reading. */
async function settle(env, want) {
  const end = Date.now() + POLL_MS;
  let got;
  do {
    got = { shard0: await shard(env, 0), shard1: await shard(env, 1), ping0: await ping(env, 0) };
    if (got.shard0 === want[0] && got.ping0 === want[1]) break;
    await sleep(300);
  } while (Date.now() < end);
  return got;
}

async function drive(env) {
  const out = {};
  await plan(env, {});
  out.whole = await settle(env, [503, 503]);
  await plan(env, { group_only: true });
  out.group = await settle(env, [200, 200]);
  await plan(env, { chains: ["release"] });
  out.chains = await settle(env, [200, 200]);
  await plan(env, null);
  out.off = await settle(env, [200, 200]);
  return out;
}

export default class extends site {
  async fetch(req) {
    if (new URL(req.url).pathname === "/drive") return Response.json(await drive(this.env));
    return super.fetch(req);
  }
}
