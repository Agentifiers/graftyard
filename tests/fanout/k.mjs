// The committed k of a repository's network on graft-m2b: read it (`ranges` on the leader), or set it through the
// leader's test-faults `/bench_seed {"fanout": k}`, which records `RepoEvent::Fanout` as `fanout` does.
//   node tests/fanout/k.mjs repo:ID [k]
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { api, leaderOf, log, member, tokens } from "../am1/hosted/lib.mjs";

const [chain, k] = process.argv.slice(2);
if (!chain?.startsWith("repo:")) throw new Error("usage: k.mjs repo:ID [k]");
// A new repository's group is created on its first read: read it once through the edge first.
const names = (await import("node:fs")).readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").trim().split("\n").map((l) => l.split(" "));
const name = names.find(([, i]) => `repo:${i}` === chain)?.[0];
if (name) await api("GET", `/api/v3/repos/m3org/${name}/branches/main`, tokens.admin);
const leader = await leaderOf(chain);
// A step commits only to caught-up pool slots (pool-warm): the first ask leases and warms them, so the ask repeats
// every 10 s until k is reached or FORCE_S (default 180) pass.
if (k !== undefined) {
  const until = Date.now() + Number(process.env.FORCE_S ?? 180) * 1000;
  for (;;) {
    const r = await member(chain, leader, "/bench_seed", { fanout: Number(k) }, 20_000);
    log(`${chain} leader ${leader}: set k ${k}: ${JSON.stringify(r.v ?? r.status).slice(0, 200)}`);
    if (r.v?.k === Number(k) || Date.now() > until) break;
    await new Promise((res) => setTimeout(res, 10_000));
  }
}
const v = await member(chain, leader, "/read", { q: { ranges: true } }, 20_000);
log(`${chain} leader ${leader}: k ${JSON.stringify(v.v?.value?.fanout ?? v.v ?? v.status).slice(0, 200)} at ${v.v?.index}`);
