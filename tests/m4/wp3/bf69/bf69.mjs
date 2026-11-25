// BF-69 on graft-m4a (build/BUILD-PLAN.md, M4-WP3; decisions/DECISIONS.md, "`actions-cache`"): an unmodified
// actions/runner 2.338.2 runs tests/m4/wp3/bf69/cache.yml, whose two jobs use actions/cache@v4 and @v5 mirrored on the
// install (tests/m4/wp3/bf69/mirror.mjs). Each case commits its `bf69.env` (KEY, RESTORE, SEED, MIB) to the branch it
// runs on, dispatches or opens a pull request, runs the jobs on ephemeral runners, and reads each job's `BF69` line from
// its log. One JSON line per case into <out>/result.jsonl.
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf69/bf69.mjs <out> <case> <runner dir>...
// Cases, in the row's order: save-main, exact-main, f-prefix, f-save, main-not-f, parallel, rel-seed, pr-rel, pr-main,
// lru (with GRAFTYARD_CACHE_LIMIT=1073741824 on the edge), sweep (after the alarm's CacheSweep and chunk delete).
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, ghs, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [out, kase, ...runnerDirs] = process.argv.slice(2);
const NAME = process.env.BF69_REPO ?? "bf69";
const R = `m3org/${NAME}`;
const admin = tokens.admin;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
// The `par` case (BF-69's parallel-save line) runs `par.yml` instead, from the `actions-mirror` org.
const WFSRC = kase === "par" ? "./par.yml" : "./cache.yml";
const WF = kase === "par" ? ".github/workflows/par.yml" : ".github/workflows/cache.yml";
const A = `/api/graftyard/v1/repos/${R}/actions`;
mkdirSync(out, { recursive: true });
const must = (what, r, ...ok) => {
  if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${what}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`);
  return r.body;
};
const gql = async (query, variables) => must("graphql", await api("POST", "/api/graphql", admin, { query, variables }));
const result = (v) => { row(join(out, "result.jsonl"), { case: kase, at: new Date().toISOString(), ...v }); log(`${kase}: ${JSON.stringify(v).slice(0, 600)}`); };

// ---- the repository, the workflow, and one branch's case file
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: false, auto_init: true });
if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
g.run(["fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*"]);
function onBranch(br, env, from = "main") {
  const base = g.run(["rev-parse", "-q", "--verify", `origin/${br}`]).ok ? `origin/${br}` : `origin/${from}`;
  g.run(["checkout", "-q", "-B", br, base]);
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, WF), Object.entries(env).reduce((t, [k, v]) => t.replaceAll(`__${k}__`, String(v)), readFileSync(new URL(WFSRC, import.meta.url), "utf8")));
  writeFileSync(join(g.wt, "bf69.env"), Object.entries(env).map(([k, v]) => `${k}=${JSON.stringify(String(v))}`).join("\n") + "\n");
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "--allow-empty", "-m", `BF-69 ${kase} on ${br}`]);
  if (!g.run(["push", "-q", "-f", "origin", `HEAD:refs/heads/${br}`]).ok) throw new Error(`push ${br}`);
  g.run(["fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*"]);
  return g.run(["rev-parse", "HEAD"]).out.trim();
}

// ---- runners: one ephemeral runner per slot at a time, started while jobs are queued
let n = 0;
const slots = runnerDirs.map((dir) => ({ dir, child: null }));
function startRunner(slot) {
  n += 1;
  const dir = join(out, `${kase}-runner-${n}`);
  mkdirSync(dir, { recursive: true });
  return (async () => {
    const key = execFileSync(jit, ["keygen", dir]).toString().trim();
    const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf69-${kase}-${n}` }), 201);
    writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
    const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf69-${kase}-${n}`]).toString();
    for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slot.dir, f), { force: true });
    for (const d of ["_diag", "_work"]) rmSync(join(slot.dir, d), { recursive: true, force: true });
    const o = join(out, `${kase}-runner-${n}.out`);
    slot.child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(o)} 2>&1`], { cwd: slot.dir, stdio: ["pipe", "ignore", "ignore"], detached: true });
    slot.child.stdin.end(config);
    log(`runner ${n} (id ${reg.runner}) in ${slot.dir}`);
  })();
}
const jobsOf = async (run) => {
  let r;
  // A 503 (a tier read the leader shed) is retried, as a client would.
  for (let i = 0; (r = await api("GET", `${A}/runs/${run}`, admin)).status === 503 && i < 12; i++) await sleep(5000);
  const v = must("run", r).value;
  return v.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
};
async function jobLog(id) {
  let text = "";
  for (let i = 0; i < 12; i++) {
    const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${id}/logs`, { headers: { authorization: `token ${admin}` } });
    text = await r.text();
    if (r.status === 200 && /Cleaning up orphan processes|Job completed|Post job cleanup/.test(text)) break;
    await sleep(5000);
  }
  writeFileSync(join(out, `${kase}-job-${id}.log`), text);
  return text;
}
/** Runs every job of `runs` to completion; each job's conclusion and its `BF69` lines. */
async function runAll(runs) {
  const t0 = Date.now();
  const done = new Map();
  for (;;) {
    const all = (await Promise.all(runs.map(async (r) => (await jobsOf(r)).map((j) => ({ ...j, run: r }))))).flat();
    row(join(out, `${kase}-polls.jsonl`), { t: Date.now() - t0, jobs: all.map((j) => [j.run, j.name, j.state]) });
    for (const j of all) if (j.state === "Completed" && !done.has(j.id)) done.set(j.id, j);
    if (all.length && all.every((j) => j.state === "Completed")) break;
    if (Date.now() - t0 > 30 * 60_000) throw new Error("jobs still not done after 30 min");
    const queued = all.filter((j) => j.state === "Queued" || j.state === "Pending").length;
    for (const slot of slots) if (queued && (!slot.child || slot.child.exitCode !== null)) await startRunner(slot);
    await sleep(5000);
  }
  const jobs = [];
  for (const j of done.values()) {
    const text = await jobLog(j.id);
    const lines = text.split("\n").filter((l) => /BF69 v[45] /.test(l) && !/echo /.test(l)).map((l) => l.replace(/^.*?(BF69 )/, "$1").trim());
    jobs.push({ run: j.run, job: j.id, name: j.name, conclusion: j.s.Completed?.conclusion, bf69: lines,
      cacheLog: text.split("\n").filter((l) => /Cache (restored|saved|not found)|Failed to save|Failed to restore|reserve|Unable to reserve|another job may be creating|Cache Size|Received \d+ of/i.test(l)).slice(0, 12) });
  }
  return { secs: Math.round((Date.now() - t0) / 1000), jobs };
}
const dispatch = async (br) => must(`dispatch ${br}`, await api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: br }), 201, 200).run;
const parse = (jobs) => Object.fromEntries(jobs.map((j) => {
  const l = j.bf69.find((x) => / (restored|made|miss)/.test(x)) ?? "";
  const m = l.match(/ (restored|made|miss)(?: sha256=(\w+))?(?: hit=(\w*))?(?: matched=(\S*))?/);
  // Neither major declares a `cache-matched-key` output, so the key restored from is the toolkit's own log line.
  const matched = (j.cacheLog.find((x) => /Cache restored from key: /.test(x)) ?? "").split("Cache restored from key: ")[1]?.trim();
  return [j.name, { conclusion: j.conclusion, what: m?.[1], sha256: m?.[2], hit: m?.[3], matched, url: (j.bf69.find((x) => /cache_url=/.test(x)) ?? "").split("cache_url=")[1] }];
}));
const saved = () => { try { return JSON.parse(readFileSync(join(out, "saved.json"), "utf8")); } catch { return {}; } };
const save = (k, v) => writeFileSync(join(out, "saved.json"), JSON.stringify({ ...saved(), [k]: v }, null, 1));
const expect = (got, want) => Object.entries(want).every(([job, w]) => Object.entries(w).every(([k, v]) => got[job]?.[k] === v));

const MIB = Number(process.env.BF69_MIB ?? 200);
switch (kase) {
  case "save-main": {
    onBranch("main", { KEY: "bf69-main-1", RESTORE: "", SEED: 1, MIB });
    const r = await runAll([await dispatch("main")]);
    const p = parse(r.jobs);
    save("main", { v4: p.v4?.sha256, v5: p.v5?.sha256 });
    result({ ok: expect(p, { v4: { what: "made", conclusion: "Success" }, v5: { what: "made", conclusion: "Success" } }), parsed: p, ...r });
    break;
  }
  case "exact-main": {
    onBranch("main", { KEY: "bf69-main-1", RESTORE: "", SEED: 0, MIB });
    const r = await runAll([await dispatch("main")]);
    const p = parse(r.jobs), s = saved().main;
    result({ ok: expect(p, { v4: { what: "restored", hit: "true", sha256: s.v4, conclusion: "Success" }, v5: { what: "restored", hit: "true", sha256: s.v5, conclusion: "Success" } }), want: s, parsed: p, ...r });
    break;
  }
  case "f-prefix": {
    onBranch("f", { KEY: "bf69-f-1", RESTORE: "bf69-main-", SEED: 0, MIB });
    const r = await runAll([await dispatch("f")]);
    const p = parse(r.jobs), s = saved().main;
    result({ ok: expect(p, { v4: { what: "restored", hit: "false", sha256: s.v4, matched: "v4-bf69-main-1", conclusion: "Success" }, v5: { what: "restored", hit: "false", sha256: s.v5, matched: "v5-bf69-main-1", conclusion: "Success" } }), want: s, parsed: p, ...r });
    break;
  }
  case "f-save": {
    onBranch("f", { KEY: "bf69-fonly-1", RESTORE: "", SEED: 1, MIB });
    const r = await runAll([await dispatch("f")]);
    const p = parse(r.jobs);
    save("f", { v4: p.v4?.sha256, v5: p.v5?.sha256 });
    result({ ok: expect(p, { v4: { what: "made", conclusion: "Success" }, v5: { what: "made", conclusion: "Success" } }), parsed: p, ...r });
    break;
  }
  case "main-not-f": {
    onBranch("main", { KEY: "bf69-fonly-1", RESTORE: "", SEED: 0, MIB });
    const r = await runAll([await dispatch("main")]);
    const p = parse(r.jobs);
    result({ ok: expect(p, { v4: { what: "miss", conclusion: "Success" }, v5: { what: "miss", conclusion: "Success" } }), parsed: p, ...r });
    break;
  }
  case "parallel": {
    // Two runs of one key at once (two jobs each, v4 and v5, so each major's key is reserved by two jobs together):
    // every job succeeds, one save per major commits, and a later read restores that save. With two runner slots the
    // runs' jobs overlap only when both are queued before either starts, so both runs are dispatched first and the
    // run passes only when each major's two jobs overlap in time.
    onBranch("p", { KEY: "bf69-par-1", RESTORE: "", SEED: 1, MIB });
    const a = await dispatch("p");
    await api("POST", `/api/v3/repos/${R}/issues`, admin, { title: `BF-69 parallel ${Date.now()}` });
    const b = await dispatch("p");
    const r = await runAll([a, b]);
    onBranch("p", { KEY: "bf69-par-1", RESTORE: "", SEED: 0, MIB });
    const check = await runAll([await dispatch("p")]);
    const p = parse(check.jobs);
    const span = (j) => { const t = readFileSync(join(out, `${kase}-job-${j.job}.log`), "utf8").split("\n").filter((l) => /^\d{4}-/.test(l)); return [Date.parse(t[0].slice(0, 23) + "Z"), Date.parse(t.at(-1).slice(0, 23) + "Z")]; };
    const per = Object.fromEntries(["v4", "v5"].map((m) => {
      const js = r.jobs.filter((j) => j.name === m);
      const [x, y] = js.map(span);
      const saved = js.filter((j) => j.cacheLog.some((l) => /Cache saved with key/.test(l))).length;
      return [m, { jobs: js.map((j) => j.job), overlap: x && y && x[0] < y[1] && y[0] < x[1], saved, made: js.map((j) => parse([j])[m]?.sha256) }];
    }));
    const ok = r.jobs.length === 4 && r.jobs.every((j) => j.conclusion === "Success")
      && ["v4", "v5"].every((m) => per[m].saved >= 1 && p[m]?.what === "restored" && per[m].made.includes(p[m].sha256));
    result({ ok, overlapped: ["v4", "v5"].every((m) => per[m].overlap), per, restored: p, check });
    break;
  }
  case "rel-seed": {
    // `rel` holds an entry `main` lacks: saved on `rel` under bf69-rel-…; `main` holds bf69-main-… from save-main.
    onBranch("rel", { KEY: "bf69-rel-1", RESTORE: "", SEED: 1, MIB });
    const r = await runAll([await dispatch("rel")]);
    const p = parse(r.jobs);
    save("rel", { v4: p.v4?.sha256, v5: p.v5?.sha256 });
    result({ ok: expect(p, { v4: { what: "made", conclusion: "Success" }, v5: { what: "made", conclusion: "Success" } }), parsed: p, ...r });
    break;
  }
  case "pr-rel":
  case "pr-main": {
    // A pull request into `rel` (or `main`) whose restore key `bf69-` matches both branches' entries: it must restore
    // its base's entry (rel's, the base scope before the default branch), and for `main` main's.
    const base = kase === "pr-rel" ? "rel" : "main";
    const head = `pr-${base}-${Date.now() % 100000}`;
    onBranch(head, { KEY: `bf69-pr-${base}-miss`, RESTORE: "bf69-", SEED: 0, MIB }, base);
    const repoNode = (await gql(`query { repository(owner: "m3org", name: "${NAME}") { id } }`)).data.repository;
    const c = await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number } } }", {
      i: { repositoryId: repoNode.id, baseRefName: base, headRefName: head, title: `BF-69 ${kase}`, body: "BF-69's base-branch scope." },
    });
    const pr = c.data.createPullRequest.pullRequest;
    log(`PR #${pr.number} ${head} -> ${base}`);
    let runs = [];
    for (let i = 0; i < 60 && !runs.length; i++) {
      await sleep(5000);
      const b = must("runs", await api("GET", `${A}/runs`, admin)); const v = b.value ?? b;
      runs = (v.runs ?? []).filter((x) => x.event === "pull_request" && x.head_ref === `refs/pull/${pr.number}/merge`).map((x) => x.id);
    }
    if (!runs.length) { result({ ok: false, why: "no pull_request run in 5 min", pr: pr.number }); break; }
    const r = await runAll(runs.slice(0, 1));
    const p = parse(r.jobs), s = saved()[base];
    const matchedPrefix = base === "rel" ? "bf69-rel-1" : "bf69-main-1";
    result({ ok: expect(p, { v4: { what: "restored", sha256: s.v4, matched: `v4-${matchedPrefix}`, conclusion: "Success" }, v5: { what: "restored", sha256: s.v5, matched: `v5-${matchedPrefix}`, conclusion: "Success" } }), pr: pr.number, want: s, parsed: p, ...r });
    break;
  }
  case "lru": {
    // GRAFTYARD_CACHE_LIMIT is 1 GiB and every entry about 200 MiB, so at most five live. A hit moves last use only
    // when the last was over CACHE_USED_EVERY_MS (1 h) ago, so inside one row last use is the commit's time and the
    // least recently used is the oldest commit (`created` is set at commit; ids are assigned at reserve, so two jobs
    // committing together keep ids out of commit order). Save two new keys on main, then: R2 holds chunks for at most
    // 1 GiB, one more entry would pass it, the evicted entries' chunks are deleted; the new keys restore and rel's
    // older entry no longer does.
    const keys = ["bf69-lru-1", "bf69-lru-2"];
    const saves = [];
    for (const k of keys) {
      onBranch("main", { KEY: k, RESTORE: "", SEED: 1, MIB });
      saves.push(await runAll([await dispatch("main")]));
    }
    const CF = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets/graft-m4a-site-a-packs/objects`;
    const r2 = (await (await fetch(`${CF}?prefix=${encodeURIComponent("actions/cache/")}&per_page=1000`, { headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` } })).json()).result ?? [];
    const byId = {};
    for (const o of r2) { const id = Number(o.key.split("/")[3]); byId[id] = (byId[id] ?? 0) + o.size; }
    const ids = Object.keys(byId).map(Number).sort((x, y) => x - y);
    const total = ids.reduce((t, id) => t + byId[id], 0);
    const probe = {};
    for (const [k, br] of [["bf69-lru-1", "main"], ["bf69-lru-2", "main"], ["bf69-rel-1", "rel"]]) {
      onBranch(br, { KEY: k, RESTORE: "", SEED: 0, MIB });
      probe[k] = parse((await runAll([await dispatch(br)])).jobs);
    }
    const what = Object.fromEntries(Object.entries(probe).map(([k, p]) => [k, [p.v4?.what, p.v5?.what]]));
    const ok = saves.every((r) => r.jobs.every((j) => j.conclusion === "Success")) && total <= 1073741824
      && total + Math.min(...Object.values(byId)) > 1073741824
      && what["bf69-lru-1"].every((w) => w === "restored") && what["bf69-lru-2"].every((w) => w === "restored")
      && what["bf69-rel-1"].every((w) => w === "miss");
    result({ ok, r2: { ids, bytes: byId, total }, what });
    break;
  }
  case "par": {
    // BF-69's parallel-save line (the lead's ruling, 2026-11-25): one dispatch fans out to four jobs, two per major
    // (`par.yml`'s matrix), every leg seeding one key per major and holding its bytes until AT, so each key's two
    // reserves come within about a second of each other. Needs four runner slots. Pass: every job succeeds, one save
    // per major commits (the other's reserve is refused, so its post step only warns), and the read after restores one
    // whole entry's bytes, equal to the bytes of the leg whose save committed. The overlap is shown from each job's
    // reserve, read in the edge's own request log (`$workers.event.request`) for `POST …/caches`, and from the
    // toolkit's lines.
    if (runnerDirs.length < 4) throw new Error("par needs four runner directories");
    // checkout reads the repository with an installation token of m3-ci limited to contents:read on this repository,
    // set as the repository secret BF69_READ through `gh secret set` (stdin), since m4a enables no job tokens.
    const t = await ghs({ repositories: [NAME], permissions: { contents: "read" } });
    if (t.status !== 201) throw new Error(`installation token: ${t.status}`);
    const host = new URL(EDGE).host;
    const gh = spawnSync("gh", ["secret", "set", "BF69_READ", "-R", `${host}/${R}`], { input: t.body.token, encoding: "utf8",
      env: { ...process.env, GH_HOST: host, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" } });
    log(`gh secret set BF69_READ -> ${gh.status} ${(gh.stderr || "").trim().slice(0, 200)}`);
    if (gh.status !== 0) throw new Error("gh secret set");
    // AT leaves time for four runners to start and reach the seed step (acquire, checkout, the cache lookup).
    const at = Math.floor(Date.now() / 1000) + Number(process.env.BF69_LEAD_S ?? 150);
    onBranch("main", { KEY: "bf69-par-2", SEED: 1, MIB, AT: at });
    const run = await dispatch("main");
    const r = await runAll([run]);
    const lines = (j) => readFileSync(join(out, `${kase}-job-${j.job}.log`), "utf8").split("\n");
    const stamp = (l) => Date.parse(l.slice(0, 23) + "Z");
    const per = Object.fromEntries(["v4", "v5"].map((m) => {
      const js = r.jobs.filter((j) => j.name.startsWith(m));
      return [m, js.map((j) => {
        const L = lines(j);
        const at = (re) => { const l = L.find((x) => re.test(x)); return l ? new Date(stamp(l)).toISOString() : null; };
        return { job: j.job, name: j.name, conclusion: j.conclusion, made: parse([{ ...j, name: m }])[m]?.sha256,
          checkout: (L.find((l) => /BF69 v[45] leg=/.test(l) && !/echo /.test(l)) ?? "").replace(/^.*?BF69/, "BF69").trim(),
          postStart: at(/Post job cleanup|Post Run actions-mirror\/cache/), saved: at(/Cache saved with key/),
          refused: (L.find((l) => /Failed to save|Unable to reserve|another job may be creating|already exists/i.test(l)) ?? "").replace(/^\S+\s/, "").slice(0, 200) || null };
      })];
    }));
    // The read after: one job per major, no seed.
    onBranch("main", { KEY: "bf69-par-2", SEED: 0, MIB, AT: 0 });
    const check = await runAll([await dispatch("main")]);
    const read = Object.fromEntries(["v4", "v5"].map((m) => {
      const js = check.jobs.filter((j) => j.name.startsWith(m));
      return [m, js.map((j) => ({ job: j.job, conclusion: j.conclusion, ...parse([{ ...j, name: m }])[m] }))];
    }));
    // F126: every cache step's runner-side span against the edge's own log of the same job's cache calls. A step over
    // 30 s with no edge call inside its gap places the hold before the edge.
    const edgeCalls = (await (await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
      method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ queryId: `bf69f126-${Date.now()}`, timeframe: { from: Date.now() - 50 * 60_000, to: Date.now() }, view: "events", limit: 500,
        parameters: { datasets: ["cloudflare-workers"], filters: [{ key: "$metadata.service", operation: "eq", type: "string", value: "graft-m4a-edge" },
          { key: "$workers.event.request.url", operation: "includes", type: "string", value: "_services/cache/" }] } }) })).json()).result?.events?.events ?? [];
    const edge = edgeCalls.map((e) => ({ t: e.timestamp, ms: e.$workers?.wallTimeMs, m: e.$workers?.event?.request?.method,
      path: (e.$workers?.event?.request?.url ?? "").replace(/^https:\/\/[^/]+/, "").replace(/\?.*/, ""), status: e.$workers?.event?.response?.status })).sort((x, y) => x.t - y.t);
    const slow = [];
    for (const j of [...r.jobs, ...check.jobs]) {
      const L = lines(j);
      const debug = L.filter((l) => /##\[debug\].*(getCacheEntry|reserveCache|commitCache|Resource Url|Attempt \d+ of|Cache Result|failed with error)/.test(l)).map((l) => l.replace(/^(\S+).*?##\[debug\]/, "$1 ").slice(0, 200));
      for (const [from, to] of [[/##\[group\]Run actions-mirror\/cache/, /Cache (restored|not found)|Failed to restore/], [/Post job cleanup|Post Run actions-mirror\/cache/, /Cache saved|Failed to save|Unable to reserve|already exists|another job/i]]) {
        const a = L.find((l) => from.test(l)), b = a && L.slice(L.indexOf(a)).find((l) => to.test(l));
        if (!a || !b || stamp(b) - stamp(a) < 30_000) continue;
        const inGap = edge.filter((e) => e.t > stamp(a) && e.t < stamp(b));
        slow.push({ job: j.job, step: from.source.slice(0, 30), from: new Date(stamp(a)).toISOString(), to: new Date(stamp(b)).toISOString(), runnerDebug: debug, edgeInGap: inGap.length, edgeAround: edge.filter((e) => Math.abs(e.t - stamp(a)) < 400_000).slice(0, 12) });
      }
    }
    writeFileSync(join(out, "par-edge-cache.json"), JSON.stringify(edge));
    // The reserves (`POST …/caches`) from the same log, read after the check run: Workers Logs lags the request by up to
    // a minute, so a query right after the four jobs found none.
    const reserves = edge.filter((e) => e.m === "POST" && /artifactcache\/caches$/.test(e.path)).map((e) => ({ at: new Date(e.t).toISOString(), status: e.status, ms: e.ms }));
    const ok = r.jobs.length === 4 && r.jobs.every((j) => j.conclusion === "Success") && check.jobs.every((j) => j.conclusion === "Success")
      && ["v4", "v5"].every((m) => per[m].filter((x) => x.saved).length === 1
        && read[m].every((x) => x.what === "restored" && x.sha256 === per[m].find((y) => y.saved).made));
    // The read token goes now: the secret, then the token itself (`DELETE /installation/token`), not left to its hour.
    const del = await api("DELETE", `/api/v3/repos/${R}/actions/secrets/BF69_READ`, admin);
    const rev = await api("DELETE", "/api/v3/installation/token", null, undefined, { authorization: `token ${t.body.token}` });
    const after = await api("GET", `/api/v3/repos/${R}`, null, undefined, { authorization: `token ${t.body.token}` });
    log(`BF69_READ: secret delete ${del.status}, token revoke ${rev.status}, the token after ${after.status}`);
    result({ ok, at: new Date(at * 1000).toISOString(), per, reserves, read, slow, token: { secretDelete: del.status, revoke: rev.status, after: after.status } });
    break;
  }
  case "chunks": {
    // The evicted entries' chunks are gone (deleted after the commit that removed them) and every live entry's tile its
    // size: R2's listing of actions/cache/<shard>/ in the owner's home bucket, against the leader's `cache_list` of each
    // key the row used. BF69_SHARD names the run's Actions shard (`actions:7100001.<n>`), from any earlier case's log.
    const shard = process.env.BF69_SHARD;
    const CF = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets/graft-m4a-site-a-packs/objects`;
    const objs = [];
    for (let cursor = ""; ; ) {
      const r = await (await fetch(`${CF}?prefix=${encodeURIComponent(`actions/cache/${shard}/`)}&per_page=1000${cursor ? `&cursor=${cursor}` : ""}`, { headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` } })).json();
      objs.push(...(r.result ?? []).map((o) => ({ key: o.key, size: o.size })));
      cursor = r.result_info?.cursor; if (!r.result_info?.is_truncated || !cursor) break;
    }
    const byId = {};
    for (const o of objs) { const id = o.key.split("/")[3]; (byId[id] ??= { n: 0, bytes: 0 }); byId[id].n++; byId[id].bytes += o.size; }
    result({ shard, objects: objs.length, ids: byId });
    break;
  }
  default:
    throw new Error(`unknown case ${kase}`);
}
for (const s of slots) if (s.child && s.child.exitCode === null) try { process.kill(-s.child.pid, "SIGTERM"); } catch {}
process.exit(0);
