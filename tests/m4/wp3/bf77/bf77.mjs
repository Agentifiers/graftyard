// BF-77 on graft-m4a (build/BUILD-PLAN.md, M4-WP3): a repository with live cache entries and artifacts is deleted, and
// each job's GITHUB_TOKEN can do what its permissions grant and nothing more.
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf77/bf77.mjs <out> <runner dir>
// Delete half: `seed.yml` (actions-mirror/cache@v4 twice, upload-artifact@v3.2.2 once) runs in `m3org/bf77-del`; the
// driver reads the shard's `repoCaches` and both R2 prefixes, deletes the repository, and polls every 30 s for 10 min.
// Pass: `repoCaches` 0 and no object under `actions/cache/<shard>/<id>/` of the seeded entries nor under
// `actions/artifacts/<shard>/<repo>/` by 10 min. Token half: `perms.yml` runs in `m3org/bf77-tok`; pass when each
// job's probes match WANT below. One JSON line goes to <out>/result.jsonl.
import { execFileSync, spawn } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { EDGE, Git, api, leaderOf, log, row, sleep, tokens, voter } from "../../../am1/hosted/lib.mjs";

const [outArg, slotDir] = process.argv.slice(2);
const out = resolve(outArg);
mkdirSync(out, { recursive: true });
const CF = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` };
const BUCKETS = ["a", "b", "c"].map((s) => `graft-m4a-site-${s}-packs`);
const admin = tokens.admin;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
const mirror = process.env.MIRROR_ORG ?? "actions-mirror";
// CACHE_MIRROR: the org holding `cache` when `actions-mirror/cache` is held Deleted (BF-69's teardown, 2026-11-25), as
// `actions-mirror2` on graft-m4a; DEL_REPO renames the delete half's repository after a first run deleted it.
// TOKEN_ONLY=1 runs the token half alone (its rerun after `bf77-routes`, 2026-11-26, the delete half having passed),
// and TOK_REPO renames its repository, since a deleted name stays held for 90 days.
const cacheMirror = process.env.CACHE_MIRROR ?? mirror;
const must = (w, r, ...ok) => { if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`); return r.body; };
const t0 = new Date();
const fails = [];
const check = (ok, what) => { log(`${ok ? "ok  " : "FAIL"} ${what}`); if (!ok) fails.push(what); };
async function objects(bucket, prefix) {
  const all = [];
  for (let cursor = ""; ; ) {
    const r = await fetch(`${CF}/r2/buckets/${bucket}/objects?prefix=${encodeURIComponent(prefix)}&per_page=1000${cursor ? `&cursor=${cursor}` : ""}`, { headers: H });
    const j = await r.json();
    all.push(...(j.result ?? []).map((o) => o.key));
    cursor = j.result_info?.cursor;
    if (!j.result_info?.is_truncated || !cursor) return all;
  }
}
const count = async (prefix) => { let n = 0; for (const b of BUCKETS) n += (await objects(b, prefix)).length; return n; };

// One repository, its workflow, one dispatched run on one ephemeral runner (as bf75.mjs); the run's job logs.
let nrun = 0;
async function runWorkflow(name, file, priv = false) {
  const R = `m3org/${name}`, A = `/api/graftyard/v1/repos/${R}/actions`, WF = ".github/workflows/w.yml";
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: priv, auto_init: true });
  if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must("create", made, 201);
  const g = new Git(R).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, WF), readFileSync(new URL(`./${file}`, import.meta.url), "utf8").replaceAll("uses: actions/cache@", `uses: ${cacheMirror}/cache@`).replaceAll("uses: actions/", `uses: ${mirror}/`));
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "--allow-empty", "-m", "BF-77"]);
  if (!g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push");
  const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: "main" }), 201, 200).run;
  log(`${name}: run ${run}`);
  let child = null, jobs = [];
  for (const t = Date.now(); ; ) {
    const r = await api("GET", `${A}/runs/${run}`, admin);
    if (r.status === 200) jobs = r.body.value.jobs;
    if (jobs.length && jobs.every((j) => Object.keys(j.state)[0] === "Completed")) break;
    if (Date.now() - t > 20 * 60_000) throw new Error(`${name}: jobs not done after 20 min`);
    if (jobs.some((j) => ["Queued", "Pending"].includes(Object.keys(j.state)[0])) && (!child || child.exitCode !== null)) {
      nrun += 1;
      const dir = join(out, `runner-${nrun}`);
      mkdirSync(dir, { recursive: true });
      const key = execFileSync(jit, ["keygen", dir]).toString().trim();
      const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf77-${nrun}` }), 201);
      writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
      const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf77-${nrun}`]).toString();
      for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slotDir, f), { force: true });
      for (const d of ["_diag", "_work"]) rmSync(join(slotDir, d), { recursive: true, force: true });
      child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(join(out, `runner-${nrun}.out`))} 2>&1`], { cwd: slotDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
      child.stdin.end(config);
    }
    await sleep(5000);
  }
  if (child && child.exitCode === null) try { process.kill(-child.pid, "SIGTERM"); } catch {}
  const logs = {};
  for (const j of jobs) {
    let text = "";
    for (let i = 0; i < 12; i++) {
      const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${j.spec.id}/logs`, { headers: { authorization: `token ${admin}` } });
      text = await r.text();
      if (r.status === 200 && /Cleaning up orphan processes|Job completed|Post job cleanup/.test(text)) break;
      await sleep(5000);
    }
    writeFileSync(join(out, `${name}-job-${j.spec.id}.log`), text);
    logs[j.spec.name] = { conclusion: j.state.Completed?.conclusion, text };
  }
  const repo = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
  return { R, run, repo, logs };
}

// ---- the delete half
const tokenOnly = process.env.TOKEN_ONLY === "1";
let del = null, shard = null, before = null, polls = [];
if (!tokenOnly) {
del = await runWorkflow(process.env.DEL_REPO ?? "bf77-del", "seed.yml");
check(Object.values(del.logs).every((j) => j.conclusion === "Success"), `seed jobs: ${JSON.stringify(Object.fromEntries(Object.entries(del.logs).map(([k, v]) => [k, v.conclusion])))}`);
const listed = must("caches", await api("GET", `/api/v3/repos/${del.R}/actions/caches`, admin), 200, 404);
shard = (await objects(BUCKETS[0], "actions/artifacts/")).find((k) => k.includes(`/${del.repo}/`))?.split("/")[2];
check(Boolean(shard), `the seeded artifact names the repository's shard: ${shard}`);
const chain = shard;
const leader = chain ? await leaderOf(chain) : null;
const caches = async () => (await voter(chain, leader, "/read", { q: { repoCaches: del.repo } })).v?.value?.repoCaches;
before = { caches: await caches(), cacheObjs: await count(`actions/cache/${shard}/`), artObjs: await count(`actions/artifacts/${shard}/${del.repo}/`) };
check(before.caches >= 2 && before.artObjs > 0, `before the delete: ${JSON.stringify(before)} (REST caches ${JSON.stringify(listed).slice(0, 120)})`);
const gone = await api("DELETE", `/api/v3/repos/${del.R}`, admin);
check(gone.status === 204, `DELETE ${del.R}: ${gone.status}`);
const at = Date.now();
let after = null;
for (;;) {
  await sleep(30_000);
  let c;
  try { c = await caches(); } catch (e) { c = `err ${String(e).slice(0, 80)}`; }
  after = { s: Math.round((Date.now() - at) / 1000), caches: c, artObjs: await count(`actions/artifacts/${shard}/${del.repo}/`), cacheObjs: await count(`actions/cache/${shard}/`) };
  polls.push(after);
  log(`after ${after.s} s: ${JSON.stringify(after)}`);
  if ((after.caches === 0 && after.artObjs === 0) || Date.now() - at > 10 * 60_000) break;
}
check(after.caches === 0 && after.artObjs === 0 && after.s <= 630, `10 min after the delete: ${JSON.stringify(after)}`);
check(after.cacheObjs <= before.cacheObjs - 2, `the seeded entries' chunks left the shard's cache prefix: ${before.cacheObjs} -> ${after.cacheObjs}`);
}

// ---- the token half
const WANT = {
  default: { read: "200", issue: "403", status: "403", artifacts: "403", push: "refused" },
  ro: { read: "200", issue: "403", status: "403", artifacts: "403", push: "refused" },
  rw: { read: "200", issue: "201", status: "201", artifacts: "403", push: "accepted" },
};
// Private, so a read the token makes is its own permission's and not a public repository's anonymous read.
const tok = await runWorkflow(process.env.TOK_REPO ?? "bf77-tok", "perms.yml", true);
const got = {};
for (const [job, j] of Object.entries(tok.logs)) {
  got[job] = {};
  for (const m of j.text.matchAll(/BF77 (\w+) (\w+) (\w+)\s*$/gm)) if (m[1] === job) got[job][m[2]] = m[3];
}
for (const job of Object.keys(WANT)) check(JSON.stringify(got[job]) === JSON.stringify(WANT[job]), `${job}: got ${JSON.stringify(got[job])}, want ${JSON.stringify(WANT[job])}`);
const branches = must("branches", await api("GET", `/api/v3/repos/${tok.R}/branches`, admin)).map((b) => b.name);
check(branches.includes("bf77-rw") && !branches.includes("bf77-ro") && !branches.includes("bf77-default"), `branches after the pushes: ${branches}`);

const ok = fails.length === 0;
const res = { ok, from: t0.toISOString(), to: new Date().toISOString(), fails, del: del && { run: del.run, repo: del.repo, shard, before, polls }, tok: { run: tok.run, repo: tok.repo, got } };
row(join(out, "result.jsonl"), res);
log(`BF-77 ${ok ? "PASS" : "FAIL"}: ${JSON.stringify(res).slice(0, 1500)}`);
log(`sheds: python3 tests/fanout/tieroff503.py ${res.from} ${res.to} graft-m4a-edge`);
process.exit(ok ? 0 : 1);
