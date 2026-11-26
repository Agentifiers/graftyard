// BF-76 on graft-m4a (build/BUILD-PLAN.md, M4-WP3; design/SHARED.md, "Toolkit hostname heuristics and SHA pins";
// decisions/DECISIONS.md, "`pin-map`" and "`actions-artifacts`", the v4 amendment). Phases, one per call:
//   source ~/.graft-m4a/env-m4a.sh
//   node tests/m4/wp3/bf76/bf76.mjs setup <out> <forks dir>   the pin map signed with the install's release key
//                                                               (~/.graft-m4a/release.key, never printed) and stored
//   node tests/m4/wp3/bf76/bf76.mjs tag <out> <runner dir>      policy {tags: true}: tag.yml by `actions/*@v4`
//   node tests/m4/wp3/bf76/bf76.mjs pin <out> <runner dir>      policy {tags: false}, no approval: pin.yml fails
//   node tests/m4/wp3/bf76/bf76.mjs approve <out> <runner dir>  policy {pin_map: <digest>}: pin.yml again passes
//   node tests/m4/wp3/bf76/bf76.mjs withdraw <out>             policy {tags: false, pin_map: null}: no approval after it
//   node tests/m4/wp3/bf76/bf76.mjs teardown <out>              the repository deleted and the policy withdrawn
// Each phase writes <out>/<phase>.json and exits 1 when a line fails. The mirrors (`actions/upload-artifact`,
// `actions/download-artifact`, `graftyard-actions/*`) are pushed by row.sh with tests/m4/wp3/bf69/mirror.mjs.
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash, createPrivateKey, sign } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { EDGE, Git, api, log, row, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [phase, outArg, dirArg] = process.argv.slice(2);
const out = resolve(outArg ?? ".");
mkdirSync(out, { recursive: true });
const NAME = process.env.BF76_REPO ?? "bf76";
const ORG = "m3org";
const R = `${ORG}/${NAME}`, A = `/api/graftyard/v1/repos/${R}/actions`;
const SUB = `/api/graftyard/v1/orgs/${ORG}/actions/substitution`;
const admin = tokens.admin;
const UP = { upstream: "actions/upload-artifact", upstream_sha: "ea165f8d65b6e75b540449e92b4886f43607fa02",
  fork: "graftyard-actions/upload-artifact", fork_sha: "aeb6043fcd99c3bdff3a1b86b98a3d66bf60897e", tag: "v4.6.2" };
const DOWN = { upstream: "actions/download-artifact", upstream_sha: "d3f86a106a0bac45b974a628896c90dbdf5c8093",
  fork: "graftyard-actions/download-artifact", fork_sha: "373906d2915ced2e53ba59693e2d2acf30171209", tag: "v4.3.0" };
const GHES_MSG = "@actions/artifact v2.0.0+, upload-artifact@v4+ and download-artifact@v4+ are not currently supported on GHES.";
const repoRoot = new URL("../../../..", import.meta.url).pathname;
const jit = process.env.JITCONFIG ?? join(process.env.CARGO_TARGET_DIR ?? join(repoRoot, "target"), "release/examples/jitconfig");
const must = (w, r, ...ok) => { if (!(ok.length ? ok : [200, 201]).includes(r.status)) throw new Error(`${w}: ${r.status} ${JSON.stringify(r.body).slice(0, 400)}`); return r.body; };
const sha = (b) => createHash("sha256").update(b).digest("hex");
const fails = [];
const lines = [];
const check = (ok, what) => { log(`${ok ? "ok  " : "FAIL"} ${what}`); lines.push({ ok, what }); if (!ok) fails.push(what); };
const t0 = new Date();
const finish = (extra = {}) => {
  const res = { phase, ok: fails.length === 0, from: t0.toISOString(), to: new Date().toISOString(), fails, lines, ...extra };
  writeFileSync(join(out, `${phase}.json`), JSON.stringify(res, null, 1));
  row(join(out, "result.jsonl"), res);
  log(`BF-76 ${phase} ${res.ok ? "PASS" : "FAIL"} (${res.from} to ${res.to})`);
  log(`sheds: python3 tests/fanout/tieroff503.py ${res.from} ${res.to} graft-m4a-edge`);
  process.exit(res.ok ? 0 : 1);
};
// A new repository's chain may answer 503 for its first minutes (hosted-run.mjs): retried, recorded, not counted.
async function retry(what, call) {
  const t = Date.now();
  for (;;) {
    const r = await call();
    if (r.status !== 503 || Date.now() - t > 600_000) return r;
    row(join(out, "retries.jsonl"), { phase, what, t: Date.now() - t, body: JSON.stringify(r.body).slice(0, 200) });
    await sleep(2000);
  }
}
const policy = async (body) => {
  const r = await retry("policy", () => api("PUT", SUB, admin, body));
  const got = await api("GET", SUB, admin);
  log(`policy PUT ${JSON.stringify(body)}: ${r.status}; now ${JSON.stringify(got.body)}`);
  return { put: r.status, now: got.body };
};

if (phase === "setup") {
  // The pin map: both pins, as the vendor's CI would derive it from build-forks.sh, signed by Keys.release.
  const doc = Buffer.from(JSON.stringify({ version: 1, pins: [UP, DOWN] }));
  const key = createPrivateKey(readFileSync(`${process.env.HOME}/.graft-m4a/release.key`));
  const sig = sign(null, doc, key);
  const digest = sha(doc);
  writeFileSync(join(out, "pin-map.json"), doc);
  writeFileSync(join(out, "pin-map.sig"), sig.toString("base64"));
  const put = await api("PUT", "/api/graftyard/v1/actions/pin-map", admin, { document: doc.toString("base64"), signature: sig.toString("base64") });
  check(put.status === 201 && put.body?.digest === digest, `PUT pin-map: ${put.status} ${JSON.stringify(put.body)}, want digest ${digest}`);
  // A map under another key is refused (the verifier is Keys.release, not anything that signs).
  const other = sign(null, doc, createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.alloc(32, 9)]), format: "der", type: "pkcs8" }));
  const bad = await api("PUT", "/api/graftyard/v1/actions/pin-map", admin, { document: doc.toString("base64"), signature: other.toString("base64") });
  check(bad.status === 422, `PUT pin-map signed by another key: ${bad.status}`);
  const anon = await api("PUT", "/api/graftyard/v1/actions/pin-map", null, { document: doc.toString("base64"), signature: sig.toString("base64") });
  check(anon.status === 401, `PUT pin-map with no token: ${anon.status}`);
  // The forks are on the install at the SHAs the map names.
  for (const p of [UP, DOWN]) {
    const c = await api("GET", `/api/v3/repos/${p.fork}/commits/v4`, null);
    check(c.status === 200 && c.body?.sha === p.fork_sha, `${p.fork}@v4 is ${c.body?.sha} (want ${p.fork_sha})`);
    const u = await api("GET", `/api/v3/repos/${p.upstream}/commits/${p.upstream_sha}`, null);
    check(u.status === 200 && u.body?.sha === p.upstream_sha, `${p.upstream}@${p.upstream_sha.slice(0, 8)} on the install: ${u.status}`);
  }
  writeFileSync(join(out, "digest.txt"), digest);
  finish({ digest });
}

// r2's first step: m3org from r1's state (tags off, the empty map approved) to no approval, by `{pin_map: null}`.
if (phase === "withdraw") {
  const before = (await api("GET", SUB, admin)).body;
  const p = await policy({ tags: false, pin_map: null });
  check(p.put === 200 && p.now?.tags === false && p.now?.pin_map === null,
    `{tags: false, pin_map: null} withdraws: before ${JSON.stringify(before)}, after ${JSON.stringify(p.now)}`);
  finish({ before, after: p.now });
}

if (phase === "teardown") {
  let p = await policy({ tags: false, pin_map: null });
  // r1: `{pin_map: null}` kept the approval (the op's JSON reads null as "keep"). Until that is fixed, approve an empty
  // map signed the same way, which approves no pin, then store the real map again as the install's current one.
  if (p.now?.pin_map) {
    const key = createPrivateKey(readFileSync(`${process.env.HOME}/.graft-m4a/release.key`));
    const store = async (d) => api("PUT", "/api/graftyard/v1/actions/pin-map", admin, { document: d.toString("base64"), signature: sign(null, d, key).toString("base64") });
    const empty = Buffer.from(JSON.stringify({ version: 1, pins: [] }));
    const e = await store(empty);
    const a = await policy({ pin_map: e.body?.digest });
    const real = await store(readFileSync(join(out, "pin-map.json")));
    log(`withdraw workaround: empty map ${e.status} ${e.body?.digest}, approved ${JSON.stringify(a.now)}, real map current again ${real.status} ${real.body?.digest}`);
    check(false, `{pin_map: null} withdrew nothing (${JSON.stringify(p.now)}); the empty map ${e.body?.digest?.slice(0, 12)} is approved instead`);
    p = a;
  }
  const d = await api("DELETE", `/api/v3/repos/${R}`, admin);
  const after = await api("GET", `/api/v3/repos/${R}`, null);
  check(p.now?.tags === false, `tag substitution off: ${JSON.stringify(p.now)}`);
  check([204, 404].includes(d.status) && after.status === 404, `${R}: DELETE ${d.status}, anonymous read after ${after.status}`);
  finish();
}

// ---- the runs
const slotDir = dirArg;
if (!slotDir) throw new Error("usage: bf76.mjs tag|pin|approve <out> <runner dir>");
const g = new Git(R).as(admin);
async function repo() {
  const made = await api("POST", `/api/v3/orgs/${ORG}/repos`, admin, { name: NAME, private: true, auto_init: true });
  if (!(made.status === 201 || (made.status === 422 && JSON.stringify(made.body).includes("already exists")))) must("create", made, 201);
  for (let i = 0; !g.clone().ok; i++) { if (i > 20) throw new Error("clone"); await sleep(3000); }
}
// `file` (tag.yml or pin.yml) on `branch`: main for the tag job; `pin` for the pinned job, whose pull request into main
// lets GraphQL reach its commit's check runs.
function put(file, branch) {
  g.run(["fetch", "-q", "origin"]);
  const has = g.run(["rev-parse", "-q", "--verify", `origin/${branch}`]).ok;
  g.run(["checkout", "-q", "-B", branch, has ? `origin/${branch}` : "origin/main"]);
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, ".github/workflows", file), readFileSync(new URL(`./${file}`, import.meta.url)));
  g.run(["add", "-A"]);
  if (g.run(["commit", "-q", "-m", `BF-76 ${file}`]).ok && !g.run(["push", "-q", "origin", `HEAD:refs/heads/${branch}`]).ok) throw new Error("push");
  return g.run(["rev-parse", "HEAD"]).out.trim();
}
let n = 0, child = null;
async function startRunner() {
  n += 1;
  const dir = join(out, `${phase}-runner-${n}`);
  mkdirSync(dir, { recursive: true });
  const key = execFileSync(jit, ["keygen", dir]).toString().trim();
  const reg = must("register", await retry("register", () => api("POST", `${A}/runners`, admin, { labels: ["self-hosted", "linux", "x64"], key, name: `bf76-${phase}-${n}` })), 201);
  writeFileSync(join(dir, "answer.json"), JSON.stringify(reg));
  const config = execFileSync(jit, ["config", dir, join(dir, "answer.json"), `bf76-${phase}-${n}`]).toString();
  for (const f of [".runner", ".credentials", ".credentials_rsaparams"]) rmSync(join(slotDir, f), { force: true });
  for (const d of ["_diag", "_work"]) rmSync(join(slotDir, d), { recursive: true, force: true });
  child = spawn("bash", ["-c", `exec ./run.sh --jitconfig "$(cat)" >> ${JSON.stringify(join(out, `${phase}-runner-${n}.out`))} 2>&1`], { cwd: slotDir, stdio: ["pipe", "ignore", "ignore"], detached: true });
  child.stdin.end(config);
  log(`runner ${n} (id ${reg.runner})`);
}
async function runWorkflow(file, ref) {
  const run = must("dispatch", await retry("dispatch", () => api("POST", `${A}/dispatch`, admin, { workflow: `.github/workflows/${file}`, ref })), 201, 200).run;
  log(`run ${run} (${file} on ${ref})`);
  let jobs;
  for (const t = Date.now(); ; ) {
    const v = must("run", await retry("run", () => api("GET", `${A}/runs/${run}`, admin))).value;
    jobs = v.jobs.map((j) => ({ id: j.spec.id, name: j.spec.name, state: Object.keys(j.state)[0], s: j.state }));
    row(join(out, `${phase}-polls.jsonl`), { t: Date.now() - t, jobs: jobs.map((j) => [j.name, j.state]) });
    if (jobs.length && jobs.every((j) => j.state === "Completed")) break;
    if (Date.now() - t > 25 * 60_000) throw new Error("jobs not done after 25 min");
    if (jobs.some((j) => j.state === "Queued") && (!child || child.exitCode !== null)) await startRunner();
    await sleep(5000);
  }
  if (child && child.exitCode === null) try { process.kill(-child.pid, "SIGTERM"); } catch {}
  const logs = {};
  for (const j of jobs) {
    let text = "";
    for (let i = 0; i < 15; i++) {
      const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${j.id}/logs`, { headers: { authorization: `token ${admin}` } });
      text = await r.text();
      if (r.status === 200 && /Cleaning up orphan processes|Complete job|Post job cleanup/.test(text)) break;
      await sleep(5000);
    }
    writeFileSync(join(out, `${phase}-job-${j.name}-${j.id}.log`), text);
    logs[j.name] = text;
  }
  return { run, jobs, logs };
}
const byName = (jobs, n) => jobs.find((j) => j.name === n);
const conclusion = (j) => j?.s?.Completed?.conclusion;
const downloadLine = (text, as, forkSha) => new RegExp(`Download action repository '${as.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&")}' \\(SHA:${forkSha}\\)`).test(text);
const unzip = (bytes) => {
  const d = mkdtempSync(join(tmpdir(), "bf76-zip-"));
  writeFileSync(join(d, "a.zip"), bytes);
  const t = spawnSync("unzip", ["-t", "-q", join(d, "a.zip")]);
  spawnSync("unzip", ["-q", "-o", join(d, "a.zip"), "-d", join(d, "x")]);
  const files = {};
  const walk = (p) => { for (const f of readdirSync(p)) { const q = join(p, f); if (statSync(q).isDirectory()) walk(q); else files[relative(join(d, "x"), q)] = sha(readFileSync(q)); } };
  try { walk(join(d, "x")); } catch {}
  rmSync(d, { recursive: true, force: true });
  return { tested: t.status === 0, files };
};
async function restZip(run, name) {
  const list = must("run artifacts", await api("GET", `/api/v3/repos/${R}/actions/runs/${run}/artifacts`, admin));
  const a = list.artifacts.find((x) => x.name === name);
  if (!a) return { listed: list.artifacts.map((x) => x.name) };
  const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/artifacts/${a.id}/zip`, { headers: { authorization: `token ${admin}` }, redirect: "manual" });
  const loc = r.headers.get("location");
  const z = loc ? await fetch(loc) : null;
  const bytes = z ? Buffer.from(await z.arrayBuffer()) : null;
  return { a, status: r.status, zstatus: z?.status, size: bytes?.length, ...(bytes ? unzip(bytes) : {}) };
}
const sumsOf = (text) => Object.fromEntries([...text.matchAll(/BF76 SUM ([0-9a-f]{64})\s+(\S+)/g)].filter((m) => !/sed /.test(m[0])).map((m) => [m[2], m[1]]));

await repo();

if (phase === "tag") {
  const p = await policy({ tags: true, pin_map: null });
  check(p.put === 200 && p.now?.tags === true, `org policy tags on: ${p.put} ${JSON.stringify(p.now)}`);
  put("tag.yml", "main");
  const { run, jobs, logs } = await runWorkflow("tag.yml", "main");
  const up = byName(jobs, "up"), down = byName(jobs, "down");
  check(conclusion(up) === "Success" && conclusion(down) === "Success", `jobs: up ${conclusion(up)}, down ${conclusion(down)}`);
  check(downloadLine(logs.up ?? "", "actions/upload-artifact@v4", UP.fork_sha), `up's download line names actions/upload-artifact@v4 and the fork's SHA ${UP.fork_sha.slice(0, 8)}`);
  check(downloadLine(logs.down ?? "", "actions/download-artifact@v4", DOWN.fork_sha), `down's download line names actions/download-artifact@v4 and the fork's SHA ${DOWN.fork_sha.slice(0, 8)}`);
  check(/BF76 ALL SUMS OK/.test(logs.down ?? "") && !/sha256sum -c.*FAILED|: FAILED/.test(logs.down ?? ""), "down: every file's SHA-256 matches (BF76 ALL SUMS OK)");
  check(!/GHESNotSupportedError/.test((logs.up ?? "") + (logs.down ?? "")), "no GHESNotSupportedError in either job");
  // The same artifact through BF-75's REST zip, against the job's own sums.
  const sums = sumsOf(logs.up ?? "");
  const z = await restZip(run, "v4art");
  writeFileSync(join(out, "tag-zip.json"), JSON.stringify({ sums, z }, null, 1));
  check(z.status === 302 && z.zstatus === 200 && z.tested && JSON.stringify(z.files, Object.keys(z.files ?? {}).sort()) === JSON.stringify(sums, Object.keys(sums).sort()),
    `REST zip of v4art: ${z.status}/${z.zstatus}, ${z.size} B, unzip -t ${z.tested}, files ${JSON.stringify(z.files)} vs sums ${JSON.stringify(sums)}`);
  const days = z.a ? (Date.parse(z.a.expires_at) - Date.parse(z.a.created_at)) / 86_400_000 : null;
  check(days !== null && Math.abs(days - 2) < 0.01, `v4art's retention-days: 2 reads as expires_at - created_at = ${days} days`);
  finish({ run, jobs: jobs.map((j) => [j.id, j.name, conclusion(j)]), sums });
}

// ---- the pinned job, before and after the approval
const head = put("pin.yml", "pin");
const pinned = `actions/upload-artifact@${UP.upstream_sha}`;
if (phase === "pin") {
  // PIN_TAGS=1: tag substitution on, still no approved map (a SHA pin is not a tag, so it still runs as written).
  const tags = process.env.PIN_TAGS === "1";
  const p = await policy({ tags, pin_map: null });
  check(p.put === 200 && p.now?.tags === tags && p.now?.pin_map === null, `org policy: tags ${tags}, no approved map: ${JSON.stringify(p.now)}`);
  const { run, jobs, logs } = await runWorkflow("pin.yml", "pin");
  const j = byName(jobs, "pinned"), text = logs.pinned ?? "";
  check(conclusion(j) === "Failure", `the pinned job concluded ${conclusion(j)} (want Failure)`);
  // The runner downloads the upstream bytes at "Set up job", as written, and the action's own step fails.
  check(downloadLine(text, pinned, UP.upstream_sha), `Set up job downloads ${pinned} at the upstream SHA (as written, no substitution)`);
  const warn = text.includes(`##[warning]Artifact upload failed with error: GHESNotSupportedError: ${GHES_MSG}`);
  const err = text.includes(`##[error]${GHES_MSG}`);
  check(warn && err, `the step's lines: ##[warning]...GHESNotSupportedError ${warn}, ##[error]${GHES_MSG.slice(0, 30)}... ${err}`);
  const stepRe = new RegExp(`##\\[group\\]Run ${pinned.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&")}`);
  const at = text.search(stepRe), errAt = text.indexOf(`##[error]${GHES_MSG}`);
  check(at >= 0 && errAt > at, `the error is in the action's own step (step at ${at}, error at ${errAt}), not at Set up job`);
  // The check run's annotations, REST then GraphQL.
  const runs = must("check-runs", await retry("check-runs", () => api("GET", `/api/v3/repos/${R}/commits/${head}/check-runs`, admin)));
  const cr = runs.check_runs.find((c) => c.id === j.id) ?? runs.check_runs.find((c) => c.name === "pinned");
  check(cr?.conclusion === "failure" && cr?.output?.annotations_count >= 2, `check run ${cr?.id}: ${cr?.status}/${cr?.conclusion}, annotations_count ${cr?.output?.annotations_count}`);
  const ann = cr ? must("annotations", await api("GET", `/api/v3/repos/${R}/check-runs/${cr.id}/annotations`, admin)) : [];
  writeFileSync(join(out, "pin-annotations.json"), JSON.stringify({ check_run: cr, annotations: ann }, null, 1));
  const ours = ann.find((a) => a.title === "Pinned action needs the Graftyard pin map");
  const link = `/organizations/${ORG}/settings/actions/pin-map`;
  check(ours?.annotation_level === "failure" && ours.message.includes(link) && ours.message.includes(`${UP.fork}@${UP.fork_sha}`) && ours.message.includes(UP.upstream_sha),
    `REST: Graftyard's annotation, failure, names ${link} and ${UP.fork}@${UP.fork_sha.slice(0, 8)}: ${JSON.stringify(ours)?.slice(0, 400)}`);
  const step = ann.find((a) => a.annotation_level === "failure" && a.message.includes(GHES_MSG));
  check(!!step, `REST: the step's own failure annotation with GHES's message: ${JSON.stringify(step)?.slice(0, 300)}`);
  // GraphQL: a pull request from `pin`, whose head commit is the run's.
  const gql = async (query, variables) => must("graphql", await retry("graphql", () => api("POST", "/api/graphql", admin, { query, variables })));
  const repoId = (await gql(`query { repository(owner: "${ORG}", name: "${NAME}") { id } }`)).data.repository.id;
  // The pull request is opened once per repository and its number kept beside the results.
  const prFile = join(out, `pr-${NAME}.txt`);
  let number = Number((() => { try { return readFileSync(prFile, "utf8"); } catch { return ""; } })()) || null;
  if (!number) {
    const made = await gql("mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { number } } }",
      { i: { repositoryId: repoId, baseRefName: "main", headRefName: "pin", title: "BF-76 pinned upload", body: "bf76.mjs" } });
    number = made.data?.createPullRequest?.pullRequest?.number;
    if (number) writeFileSync(prFile, String(number));
    else log(`createPullRequest: ${JSON.stringify(made).slice(0, 300)}`);
  }
  const q = `query($o: String!, $n: String!, $p: Int!) { repository(owner: $o, name: $n) { pullRequest(number: $p) { commits(last: 1) { nodes { commit {
    oid statusCheckRollup { contexts(first: 20) { nodes { ... on CheckRun { name conclusion annotations(first: 50) { totalCount nodes {
      annotationLevel path title message location { start { line } end { line } } } } } } } } } } } } } }`;
  let gv, contexts = [];
  for (let i = 0; i < 6; i++) {
    gv = await gql(q, { o: ORG, n: NAME, p: number });
    contexts = gv.data?.repository?.pullRequest?.commits?.nodes?.[0]?.commit?.statusCheckRollup?.contexts?.nodes ?? [];
    if (contexts.some((c) => c.name === "pinned")) break;
    await sleep(5000);
  }
  writeFileSync(join(out, "pin-graphql.json"), JSON.stringify(gv, null, 1));
  const gc = contexts.find((c) => c.name === "pinned");
  const gOurs = gc?.annotations?.nodes?.find((a) => a.title === "Pinned action needs the Graftyard pin map");
  check(gOurs?.annotationLevel === "FAILURE" && gOurs.message.includes(link) && gOurs.message.includes(UP.fork_sha),
    `GraphQL CheckRun.annotations on PR #${number} (commit ${gv.data?.repository?.pullRequest?.commits?.nodes?.[0]?.commit?.oid?.slice(0, 8)}): ${gc?.annotations?.totalCount} annotations, ours ${JSON.stringify(gOurs)?.slice(0, 300)}`);
  check(!!gc?.annotations?.nodes?.find((a) => a.annotationLevel === "FAILURE" && a.message.includes(GHES_MSG)), "GraphQL: the step's failure annotation too");
  finish({ run, head, pr: number, job: j?.id, check_run: cr?.id, annotations: ann.length });
}

if (phase === "approve") {
  const digest = readFileSync(join(out, "digest.txt"), "utf8").trim();
  const p = await policy({ pin_map: digest });
  check(p.put === 200 && p.now?.pin_map === digest && p.now?.tags === false, `org admin approves the map ${digest.slice(0, 12)}: ${JSON.stringify(p.now)}`);
  const bad = await api("PUT", SUB, admin, { pin_map: "00".repeat(32) });
  check(bad.status === 422, `approving a digest the install does not hold: ${bad.status}`);
  const { run, jobs, logs } = await runWorkflow("pin.yml", "pin");
  const j = byName(jobs, "pinned"), text = logs.pinned ?? "";
  check(conclusion(j) === "Success", `the same pinned workflow concluded ${conclusion(j)} (want Success)`);
  check(downloadLine(text, pinned, UP.fork_sha), `Set up job downloads ${pinned.slice(0, 32)}... as written at the fork's SHA ${UP.fork_sha.slice(0, 8)}`);
  check(!/GHESNotSupportedError/.test(text), "no GHESNotSupportedError");
  const sums = sumsOf(text);
  const z = await restZip(run, "pinned");
  writeFileSync(join(out, "approve-zip.json"), JSON.stringify({ sums, z }, null, 1));
  check(z.status === 302 && z.zstatus === 200 && z.tested && z.files?.["x.bin"] === sums["p/x.bin"], `REST zip of pinned: files ${JSON.stringify(z.files)} vs ${JSON.stringify(sums)}`);
  finish({ run, head, job: j?.id });
}
throw new Error("phase: setup withdraw tag pin approve teardown");
