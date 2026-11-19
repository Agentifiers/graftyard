// G11's hosted rehome on graft-m3f: one seeded repository (b034seed.mjs, git/git's first-parent history) is rehomed
// on site <to> by `/move` (transfer.mjs's loop) with site <to>'s git cache shard torn down first, so the target is cold
// and `move_step`'s `Home` step has to hold on it (`graftyard_identity::group_move::warm_site`, `transfer_gate`).
// MergeQueue merges run one at a time, at most 10 a minute, from 30 s before the move until 90 s after the hand-off,
// each a pre-opened PR whose head is the next commit of the plan's `# future` line, so the minute after the hand-off
// is compared with m3fr7's 5 `warming` answers of 5. Then 30 s idle and every voter's `/status` commit (R29).
//   node tests/fanout/g11.mjs <repo name> <plan file> <bare dir> [to 2] [prs 24]   -> stdout, one JSON line per step
// Environment as tests/am1/hosted/lib.mjs. The token goes only to git's credential helper and headers, never argv.
import { readFileSync } from "node:fs";
import { Git, api, bedGet, leaderOf, member, sleep, statuses, tokens } from "../am1/hosted/lib.mjs";
import { serverClock } from "./srvclock.mjs";

const [name, planFile, bare, toArg = "2", nArg = "24"] = process.argv.slice(2);
if (!name || !planFile || !bare) throw new Error("usage: g11.mjs <repo name> <plan> <bare> [to] [prs]");
const to = Number(toArg);
const R = `m3org/${name}`;
const id = readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
if (!id) throw new Error(`${name}: no id`);
const chain = `repo:${id}`;
const clk = serverClock();
await clk.ready;
const out = (step, v = {}) => console.log(JSON.stringify({ at: clk.iso(), step, ...v }));
out("start", { chain, clock: clk.offset() });
const admin = tokens.admin;
const future = readFileSync(planFile, "utf8").split("\n").find((l) => l.startsWith("# future "))?.split(" ").slice(2) ?? [];
const g = new Git(R).as(admin);

// PRs: branch g11-i at future[i], opened against main.
const repoId = (await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${name}") { id } }` })).body?.data?.repository?.id;
const prs = [];
for (let i = 0; i < Math.min(Number(nArg), future.length); i++) {
  const ref = `refs/heads/g11-${i}`;
  let p = { ok: false };
  for (let a = 0; a < 4 && !p.ok; a++) {
    p = g.run(["push", "-q", g.url, `${future[i]}:${ref}`], bare);
    if (!p.ok) await sleep(5_000);
  }
  if (!p.ok) { out("branch push failed", { i, out: p.out.slice(-160) }); continue; }
  const r = await api("POST", "/api/graphql", admin, { query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
    variables: { i: { repositoryId: repoId, baseRefName: "main", headRefName: `g11-${i}`, title: `g11 ${i}`, body: "g11" } } });
  const n = r.body?.data?.createPullRequest?.pullRequest?.number;
  if (n) prs.push(n); else out("pr open failed", { i, status: r.status, body: JSON.stringify(r.body).slice(0, 160) });
}
out("prs", { n: prs.length });

// The old home's shard warm before the baseline merges.
const before = await statuses(chain);
out("before", { roles: before.map((s) => [s.site, s.role, s.term, s.commit, s.config?.config?.normal?.home ?? null]) });
const home0 = before.find((s) => s.role === "leader")?.site ?? 0;
let warm0 = { status: 0 };
for (let i = 0; i < 10 && warm0.status !== 200; i++) {
  warm0 = await bedGet(`/_bed/catchup?repo=${id}&shard=${home0}&wait=60`).catch((e) => ({ status: 0, body: String(e) }));
  out("old home warm", { site: home0, status: warm0.status });
}

// Merges, one at a time, at most one every 6 s, until `stopAt`.
let stopAt = Infinity, handoffAt = null, k = 0;
const merges = [];
const mergeLoop = (async () => {
  while (Date.now() < stopAt && k < prs.length) {
    const s = Date.now(), at = clk.iso();
    const m = await api("PUT", `/api/v3/repos/${R}/pulls/${prs[k]}/merge`, admin, { merge_method: "merge" });
    const row = { at, pr: prs[k], status: m.status, ms: Date.now() - s, msg: String(m.body?.message ?? "").slice(0, 90),
      timing: m.headers["server-timing"] ?? null, shard: m.headers["x-graft-shard-timing"] ?? null,
      sinceHandoffS: handoffAt ? +((s - handoffAt) / 1000).toFixed(1) : null };
    merges.push(row);
    out("merge", row);
    if (m.status === 200) k++;
    await sleep(Math.max(0, 6_000 - (Date.now() - s)));
  }
})();
await sleep(30_000);

// The move: transfer.mjs's loop, each answer timed, right after the target's shard is torn down. `handoffS` is when an
// ask first found the leader on <to>, within one 3 s pause of the hand-off; terms-<tag>-g11.jsonl has it to the second.
const kill = await bedGet(`/_bed/kill?repo=${id}&shard=${to}`).catch((e) => ({ status: 0, body: String(e) }));
out("target torn down", { site: to, status: kill.status, body: kill.body.slice(0, 120) });
const target = { name: "Tri", sites: [0, 1, 2], home: to, generation: 1, moving_from: null, at: 0 };
const t0 = Date.now();
let held = 0, done = false, last = null, firstHeld = null;
const until = t0 + 300_000;
while (Date.now() < until && !done) {
  const l = await leaderOf(chain);
  // The hand-off: the first ask that finds the leader on <to> (the core hands off by itself once `Home` commits).
  if (l === to && !handoffAt) handoffAt = Date.now();
  const s = Date.now();
  const r = await member(chain, l, "/move", { to: target }, 60_000);
  const v = r.v ?? { status: r.status };
  // `move_step` answers {step, ok | error, config, ms}; the wait's fields are under `ok`.
  const ok = v?.ok ?? {};
  out("move", { on: l, ms: Date.now() - s, sinceStartS: +((s - t0) / 1000).toFixed(1), step: v?.step ?? null,
    ok: JSON.stringify(v?.ok ?? null).slice(0, 200), error: v?.error ? JSON.stringify(v.error).slice(0, 200) : null });
  if (ok.warming) { held++; firstHeld ??= s; }
  else if (v?.ok && v.step !== "Done" && !last) last = { step: v.step, at: s, ...ok };
  done = v?.step === "Done";
  if (!done) await sleep(3_000);
}
out("move end", { done, held, step: last?.step ?? null, waitedMs: last?.waitedMs ?? null, cold: last?.cold ?? false,
  heldS: firstHeld && last ? +((last.at - firstHeld) / 1000).toFixed(1) : null,
  totalS: +((Date.now() - t0) / 1000).toFixed(1), handoffS: handoffAt ? +((handoffAt - t0) / 1000).toFixed(1) : null });
const sh = await bedGet(`/_bed/shard_status?repo=${id}&shard=${to}`).catch((e) => ({ status: 0, body: String(e) }));
out("target shard after hand-off", { status: sh.status, body: sh.body.slice(0, 300) });
stopAt = (handoffAt ?? Date.now()) + 90_000;
await mergeLoop;
const after = merges.filter((m) => m.sinceHandoffS !== null && m.sinceHandoffS <= 60);
out("merges", { all: merges.length, ok: merges.filter((m) => m.status === 200).length,
  warming: merges.filter((m) => /warming/.test(m.msg)).length,
  minuteAfter: { n: after.length, ok: after.filter((m) => m.status === 200).length, warming: after.filter((m) => /warming/.test(m.msg)).length,
    p50ms: after.map((m) => m.ms).sort((a, b) => a - b)[Math.floor(after.length / 2)] ?? null, maxMs: Math.max(0, ...after.map((m) => m.ms)) } });

// R29: after 30 s with no write, every voter's commit against the leader's.
await sleep(30_000);
const idle = await statuses(chain);
const lead = idle.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
out("idle", { roles: idle.map((s) => [s.site, s.role, s.term, s.commit, s.applied, s.config?.config?.normal?.home ?? null]),
  followersAtCommit: idle.filter((s) => s.role !== "leader").every((s) => s.commit === lead?.commit) });
out("rehomed", { ok: done && lead?.site === to });
