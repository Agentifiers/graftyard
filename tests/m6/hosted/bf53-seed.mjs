// BF-53's repository on graft-m3f (`tests/m6/results/bf53.md`), made while the feed is held so the feed never carries
// it from index 1: `m3org/bf53-<tag>` with auto_init, STORED pushes of a 3 MiB file each (over INLINE_MAX, so each
// places a stored pack) and INLINE small ones, a branch `f` and a pull request through GraphQL `createPullRequest` on
// the main host. Then the archive cap is lowered on every member (test-faults `/bench_seed {"archiveCap"}`, re-set
// every 15 s because it lives in the isolate) until the leader has a `Promoted`, an `Archived`, and a stored log that
// no longer starts at 1 (`truncatedTo`), and the cap is cleared. Writes <out>/seed.json and one JSON line per step to
// <out>/seed.jsonl. Tokens ride in headers and a credential helper only (`lib.mjs`).
//   node tests/m6/hosted/bf53-seed.mjs <tag> <out dir>        (env as tests/am1/lf13/env-m3f.sh, TOOLS_TOKEN_FILE)
//   node tests/m6/hosted/bf53-seed.mjs push <out dir>         one more stored push to `main`, appended to seed.json
import { randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, log, member, row, sleep, statuses, tokens, voterNames } from "../../am1/hosted/lib.mjs";

const STORED = Number(process.env.STORED ?? 6), INLINE = Number(process.env.INLINE ?? 2);
const push = (g, file, bytes, ref = "main") => {
  writeFileSync(join(g.wt, file), randomBytes(bytes));
  g.run(["add", file]);
  g.run(["commit", "-q", "-m", `bf53 ${file}`]);
  const t0 = Date.now();
  const p = g.run(["push", "-q", "origin", `HEAD:${ref}`]);
  return { file, bytes, ref, ok: p.ok, ms: Date.now() - t0, out: p.ok ? "" : p.out.slice(0, 300) };
};

if (process.argv[2] === "push") {
  const OUT = process.argv[3];
  const s = JSON.parse(readFileSync(`${OUT}/seed.json`));
  const g = new Git(s.repo).as(tokens.admin);
  for (let i = 0; i < 5 && !g.clone().ok; i++) await sleep(3000);
  const p = push(g, `late-${s.late.length}.bin`, 3 << 20);
  s.late.push({ ...p, head: g.remoteHead("refs/heads/main") });
  writeFileSync(`${OUT}/seed.json`, JSON.stringify(s), { mode: 0o600 });
  row(`${OUT}/seed.jsonl`, { step: "late push", ...p, at: new Date().toISOString() });
  log(`late push ${p.ok ? "ok" : p.out}`);
  process.exit(p.ok ? 0 : 1);
}

const [TAG, OUT] = process.argv.slice(2);
const J = `${OUT}/seed.jsonl`;
const name = `bf53-${TAG}`;
const made = await api("POST", "/api/v3/orgs/m3org/repos", tokens.admin, { name, private: true, auto_init: true });
if (made.status !== 201) throw new Error(`create ${name}: ${made.status} ${JSON.stringify(made.body).slice(0, 200)}`);
const id = made.body.id, CHAIN = `repo:${id}`;
row(J, { step: "create", repo: `m3org/${name}`, id, node_id: made.body.node_id, ms: made.ms });
log(`created m3org/${name} = ${CHAIN}`);
const g = new Git(`m3org/${name}`).as(tokens.admin);
let r;
for (let i = 0; i < 5 && !(r = g.clone()).ok; i++) await sleep(3000);
if (!r.ok) throw new Error(`clone: ${r.out}`);
const pushes = [];
for (let i = 0; i < STORED + INLINE; i++) {
  const p = i < STORED ? push(g, `big-${i}.bin`, 3 << 20) : push(g, `small-${i}.txt`, 2_000);
  pushes.push(p);
  row(J, { step: "push", ...p });
  if (!p.ok) throw new Error(`push ${p.file}: ${p.out}`);
}
// The pull request: a branch `f`, then `createPullRequest` as `gh pr create` sends it.
pushes.push(push(g, "pr.txt", 500, "refs/heads/f"));
const pr = await api("POST", "/api/graphql", tokens.admin, {
  query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number title } } }",
  variables: { i: { repositoryId: made.body.node_id, baseRefName: "main", headRefName: "f", title: "bf53 lifeboat pr", body: "read me from the lifeboat" } },
});
row(J, { step: "pull request", status: pr.status, out: pr.body?.data ?? pr.body });
if (!pr.body?.data?.createPullRequest?.pullRequest?.number) throw new Error(`createPullRequest: ${JSON.stringify(pr.body).slice(0, 300)}`);

// The archive: the cap on every voter until the leader promoted, archived and truncated, then cleared.
const stats = async (site, names) => (await member(CHAIN, site, "/stats", undefined, 15_000, names[site]).catch(() => ({}))).v ?? {};
const t0 = Date.now();
let names = {}, done = null, last = 0;
while (Date.now() - t0 < 25 * 60_000) {
  names = { ...names, ...(await voterNames(CHAIN).catch(() => ({}))) };
  const caps = await Promise.all([0, 1, 2].map((s) => member(CHAIN, s, "/bench_seed", { archiveCap: { max: 1_024, range: 65_536 } }, 15_000, names[s])
    .then((x) => x.v?.archiveCap ?? null, (e) => String(e).slice(0, 40))));
  const st = await statuses(CHAIN);
  const leader = st.find((m) => m.role === "leader")?.site;
  const ls = leader === undefined ? {} : await stats(leader, names);
  if (Date.now() - last > 60_000) {
    last = Date.now();
    row(J, { step: "cap", s: Math.round((Date.now() - t0) / 1000), caps, leader, truncatedTo: ls.truncatedTo, archiveSteps: ls.archiveSteps, alarms: (ls.alarms ?? []).slice(-2) });
  }
  if (ls.truncatedTo > 1 && ls.archiveSteps > 0) { done = { leader, truncatedTo: ls.truncatedTo, archiveSteps: ls.archiveSteps, s: Math.round((Date.now() - t0) / 1000) }; break; }
  await sleep(15_000);
}
const uncap = await Promise.all([0, 1, 2].map((s) => member(CHAIN, s, "/bench_seed", { archiveCap: {} }, 15_000, names[s])
  .then((x) => x.v?.archiveCap ?? null, (e) => String(e).slice(0, 40))));
const after = await Promise.all([0, 1, 2].map(async (s) => ({ site: s, ...(({ truncatedTo, archiveSteps }) => ({ truncatedTo, archiveSteps }))(await stats(s, names)) })));
const st = await statuses(CHAIN);
row(J, { step: "archived", done, uncap, members: after, commits: st.map((m) => [m.site, m.role, m.commit]) });
writeFileSync(`${OUT}/seed.json`, JSON.stringify({ repo: `m3org/${name}`, id, chain: CHAIN, pushes, late: [], archived: done,
  commit: Math.max(...st.map((m) => m.commit ?? 0)), head: g.remoteHead("refs/heads/main") }), { mode: 0o600 });
log(done ? `archived: ${JSON.stringify(done)}` : "no archive within 25 min");
process.exit(done ? 0 : 2);
