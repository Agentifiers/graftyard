// BF-75 on graft-m4a (build/BUILD-PLAN.md, M4-WP3; decisions/DECISIONS.md, "`actions-artifacts`", the amendment): an
// unmodified actions/runner 2.338.2 runs tests/m4/wp3/bf75/art.yml with upload-artifact@v3.2.2 mirrored under
// `actions-mirror`, then this driver reads the REST routes as a user would:
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf75/bf75.mjs <out> <runner dir>
// Lines: both lists name both artifacts with GHES's fields and the finalized sizes; the zip (302, then the URL) and
// `gh run download` give files SHA-256 identical to the job's own sums; `short`'s `expires_at` is its `created_at`
// plus one day and `keep`'s plus 90; the zip URL answers 403 after a minute; a DELETE answers 204, removes `short`
// from both lists, and leaves no object of it in R2. One JSON line goes to <out>/result.jsonl.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { EDGE, Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [outArg, slotDir] = process.argv.slice(2);
const out = resolve(outArg);
const CF = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` };
const BUCKETS = ["a", "b", "c"].map((s) => `graft-m4a-site-${s}-packs`);
const NAME = process.env.BF75_REPO ?? "bf75";
const R = `m3org/${NAME}`, A = `/api/graftyard/v1/repos/${R}/actions`, WF = ".github/workflows/art.yml";
const admin = tokens.admin;
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
mkdirSync(out, { recursive: true });
const must = (w, r, ...ok) => { if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`); return r.body; };
const sha = (b) => createHash("sha256").update(b).digest("hex");
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

// ---- the repository and the workflow, `uses:` pointed at the mirror
const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: false, auto_init: true });
if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must("create", made, 201);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
const mirror = process.env.MIRROR_ORG ?? "actions-mirror";
writeFileSync(join(g.wt, WF), readFileSync(new URL("./art.yml", import.meta.url), "utf8").replaceAll("uses: actions/", `uses: ${mirror}/`));
g.run(["add", "-A"]);
g.run(["commit", "-q", "--allow-empty", "-m", "BF-75"]);
if (!g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push");

// ---- one ephemeral runner (as bf71.mjs)
async function runner() {
  const dir = join(out, "runner-1");
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: "bf75-1" }), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), "bf75-1"]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slotDir, f), { force: true });
  for (const d of ["_diag", "_work"]) rmSync(join(slotDir, d), { recursive: true, force: true });
  const child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(join(out, "runner-1.out"))} 2>&1`], { cwd: slotDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  child.stdin.end(config);
  return child;
}
const run = must("dispatch", await api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: "main" }), 201, 200).run;
log(`run ${run}`);
const child = await runner();
let job;
for (const t = Date.now(); ; ) {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  if (r.status === 200) {
    job = r.body.value.jobs[0];
    if (job && Object.keys(job.state)[0] === "Completed") break;
  }
  if (Date.now() - t > 20 * 60_000) throw new Error("the job is not done after 20 min");
  await sleep(5000);
}
try { process.kill(-child.pid, "SIGTERM"); } catch {}
check(job.state.Completed?.conclusion === "Success", `job ${job.spec.id} concluded ${job.state.Completed?.conclusion}`);
let text = "";
for (let i = 0; i < 12; i++) {
  const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.spec.id}/logs`, { headers: { authorization: `token ${admin}` } });
  text = await r.text();
  if (r.status === 200 && /BF75 SUM short/.test(text)) break;
  await sleep(5000);
}
writeFileSync(join(out, "job.log"), text);
const sums = {};
for (const m of text.matchAll(/BF75 SUM (\S+) ([0-9a-f]{64})\s+(\S+)/g)) (sums[m[1]] ??= {})[m[3]] = m[2];
check(Object.keys(sums.keep ?? {}).length === 3 && Object.keys(sums.short ?? {}).length === 1, `the job's sums: ${JSON.stringify(sums)}`);

// ---- the lists, both with GHES's fields
const FIELDS = ["archive_download_url", "created_at", "expired", "expires_at", "id", "name", "node_id", "size_in_bytes", "updated_at", "url", "workflow_run"];
const repoList = must("list", await api("GET", `/api/v3/repos/${R}/actions/artifacts?per_page=100`, admin));
const runList = must("run list", await api("GET", `/api/v3/repos/${R}/actions/runs/${run}/artifacts`, admin));
writeFileSync(join(out, "lists.json"), JSON.stringify({ repoList, runList }, null, 1));
const byName = (l) => Object.fromEntries(l.artifacts.filter((a) => a.workflow_run?.id === run).map((a) => [a.name, a]));
for (const [w, l] of [["repo", repoList], ["run", runList]]) {
  const n = byName(l);
  check(n.keep && n.short && Object.keys(n).length === 2, `${w} list names ${Object.keys(n)} (total_count ${l.total_count})`);
  for (const a of Object.values(n)) check(JSON.stringify(Object.keys(a).sort()) === JSON.stringify(FIELDS) && a.expired === false, `${w} ${a.name}: fields ${Object.keys(a).sort()}, expired ${a.expired}`);
}
const art = byName(runList);
const wantSize = { keep: 3000000 + 9000000, short: 4000 };
for (const n of ["keep", "short"]) check(art[n]?.size_in_bytes === wantSize[n], `${n} size_in_bytes ${art[n]?.size_in_bytes}, want ${wantSize[n]}`);
const days = (a) => (Date.parse(a.expires_at) - Date.parse(a.created_at)) / 86_400_000;
check(days(art.short) === 1, `short expires_at ${art.short.expires_at} is created_at ${art.short.created_at} + ${days(art.short)} days`);
check(days(art.keep) === 90, `keep expires_at is created_at + ${days(art.keep)} days`);
check(Math.abs(Date.parse(art.short.created_at) - t0) < 30 * 60_000, "short was created during this run");
const one = must("get", await api("GET", `/api/v3/repos/${R}/actions/artifacts/${art.keep.id}`, admin));
check(JSON.stringify(one) === JSON.stringify(art.keep), "GET artifacts/{id} answers the list's row");

// ---- the zip: 302 to a URL with no token, which streams the files; after a minute, the URL is refused
const unzip = (bytes) => {
  const d = mkdtempSync(join(tmpdir(), "bf75-zip-"));
  writeFileSync(join(d, "a.zip"), bytes);
  const t = spawnSync("unzip", ["-t", "-q", join(d, "a.zip")]);
  spawnSync("unzip", ["-q", "-o", join(d, "a.zip"), "-d", join(d, "x")]);
  const files = {};
  const walk = (p) => { for (const f of readdirSync(p)) { const q = join(p, f); if (statSync(q).isDirectory()) walk(q); else files[relative(join(d, "x"), q)] = sha(readFileSync(q)); } };
  walk(join(d, "x"));
  return { tested: t.status === 0, files };
};
const zipOf = async (a) => {
  const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/artifacts/${a.id}/zip`, { headers: { authorization: `token ${admin}` }, redirect: "manual" });
  const loc = r.headers.get("location");
  const z = loc ? await fetch(loc) : null;
  return { status: r.status, loc, zstatus: z?.status, bytes: z ? Buffer.from(await z.arrayBuffer()) : null };
};
let lateLoc;
for (const n of ["keep", "short"]) {
  const z = await zipOf(art[n]);
  const u = z.bytes ? unzip(z.bytes) : { tested: false, files: {} };
  check(z.status === 302 && z.zstatus === 200 && u.tested, `${n} zip: ${z.status}, url ${z.zstatus}, ${z.bytes?.length} B, unzip -t ${u.tested}`);
  check(JSON.stringify(u.files, Object.keys(u.files).sort()) === JSON.stringify(sums[n], Object.keys(sums[n] ?? {}).sort()), `${n} zip files SHA-256 identical: ${JSON.stringify(u.files)}`);
  if (n === "short") lateLoc = z.loc;
}

// ---- gh run download, as a user runs it
const ghDir = mkdtempSync(join(tmpdir(), "bf75-gh-"));
const host = new URL(EDGE).host;
const ghEnv = { ...process.env, GH_CONFIG_DIR: ghDir, GH_HOST: host, GH_ENTERPRISE_TOKEN: admin, GH_NO_UPDATE_NOTIFIER: "1", GH_PROMPT_DISABLED: "1" };
delete ghEnv.GH_TOKEN; delete ghEnv.GITHUB_TOKEN;
const dl = join(out, "gh-download");
rmSync(dl, { recursive: true, force: true });
const gh = spawnSync("gh", ["run", "download", String(run), "-R", `${host}/${R}`, "-D", dl], { env: ghEnv, encoding: "utf8" });
writeFileSync(join(out, "gh.txt"), `${gh.status}\n${gh.stdout}\n${gh.stderr}`.replaceAll(admin, "<token>"));
const got = {};
const walk = (p, base) => { for (const f of readdirSync(p)) { const q = join(p, f); if (statSync(q).isDirectory()) walk(q, base); else { const [a, ...rest] = relative(base, q).split("/"); (got[a] ??= {})[rest.join("/")] = sha(readFileSync(q)); } } };
try { walk(dl, dl); } catch {}
check(gh.status === 0 && JSON.stringify(got.keep) === JSON.stringify(sums.keep, Object.keys(got.keep ?? {})) && got.short?.["s.bin"] === sums.short?.["s.bin"],
  `gh run download exit ${gh.status}: ${JSON.stringify(got)}`);

// ---- the zip URL after its minute
await sleep(65_000);
const late = await fetch(lateLoc);
check(late.status === 403, `the zip URL after 65 s answers ${late.status}`);

// ---- the delete: 204, gone from both lists, no object left of it, and `keep` untouched
const repoId = must("repo", await api("GET", `/api/v3/repos/${R}`, admin)).id;
const before = {};
for (const b of BUCKETS) before[b] = (await objects(b, "actions/artifacts/")).filter((k) => k.includes(`/${repoId}/`));
const del = await api("DELETE", `/api/v3/repos/${R}/actions/artifacts/${art.short.id}`, admin);
check(del.status === 204, `DELETE short: ${del.status}`);
const after1 = byName(must("list", await api("GET", `/api/v3/repos/${R}/actions/artifacts`, admin)));
const after2 = byName(must("run list", await api("GET", `/api/v3/repos/${R}/actions/runs/${run}/artifacts`, admin)));
check(!after1.short && !after2.short && after1.keep && after2.keep, `after the delete: repo list ${Object.keys(after1)}, run list ${Object.keys(after2)}`);
const gone = await api("GET", `/api/v3/repos/${R}/actions/artifacts/${art.short.id}`, admin);
check(gone.status === 404, `GET the deleted artifact: ${gone.status}`);
const left = {};
let shortLeft = 0, keepLeft = 0;
for (const b of BUCKETS) {
  const ks = (await objects(b, "actions/artifacts/")).filter((k) => k.includes(`/${repoId}/`));
  left[b] = { before: before[b].length, after: ks.length };
  const rel = (k) => k.split(`/${run}/`)[1] ?? "";
  shortLeft += ks.filter((k) => /^(m|c)\/short$|^(i)\/short\/|^u\/\d+\/short\/|^f\/\d+\/\d+\/short\//.test(rel(k)) || k.endsWith(`/a/${art.short.id}`)).length;
  keepLeft += ks.filter((k) => /^i\/keep\//.test(rel(k))).length;
}
check(shortLeft === 0 && keepLeft === 3, `R2 after the delete: short's objects ${shortLeft}, keep's records ${keepLeft}, ${JSON.stringify(left)}`);
const unauth = await fetch(`${EDGE}/api/v3/repos/${R}/actions/artifacts/${art.keep.id}`, { method: "DELETE" });
check(unauth.status === 401, `DELETE with no token: ${unauth.status}`);

const ok = fails.length === 0;
const res = { ok, run, repo: repoId, from: t0.toISOString(), to: new Date().toISOString(), fails, sums, ids: { keep: art.keep.id, short: art.short.id } };
row(join(out, "result.jsonl"), res);
log(`BF-75 ${ok ? "PASS" : "FAIL"}: ${JSON.stringify(res).slice(0, 1500)}`);
log(`sheds: python3 tests/fanout/tieroff503.py ${res.from} ${res.to} graft-m4a-edge`);
process.exit(ok ? 0 : 1);
