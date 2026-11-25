// BF-70's install half on graft-m4a (`tests/m4/wp3/oidc/hosted-row.sh` calls it; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/oidc/hosted.mjs action <configure-aws-credentials checkout> <out dir>
//   node tests/m4/wp3/oidc/hosted.mjs job <runner dir> <out dir> <tag> [fork]
//   node tests/m4/wp3/oidc/hosted.mjs unmirror - <out dir>
// `action` creates the public repository `m3org/bf70-cac` and pushes configure-aws-credentials v6.2.0's tree to it as
// `main` and tag `v6.2.0` (the install resolves `uses:` only in its own repositories, F90), and records the tree hash,
// which must equal upstream's; `unmirror` deletes it at teardown. `job` dispatches `hosted.yml` (or, with `fork`,
// `fork.yml` as a fork pull request's run) on an unmodified runner through `../hosted-run.mjs`, reads every job's
// log, and writes `<out>/job-<tag>.json` with what each check saw. It exits 1 when a check fails. It never prints a
// token: the workflow prints only the kid, the published kids and the claims, and the log scan fails on anything
// shaped like a JWT.
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, log, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [phase, a1, out, tag, mode] = process.argv.slice(2);
const admin = tokens.admin;
mkdirSync(out, { recursive: true });
const result = (k, v) => writeFileSync(join(out, `${k}.json`), JSON.stringify(v, null, 2));

if (phase === "action") {
  const cac = a1;
  const name = "bf70-cac";
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: false, auto_init: false });
  log(`repository m3org/${name}: ${made.status}`);
  const g = new Git(`m3org/${name}`).as(admin);
  const tree = execFileSync("git", ["-C", cac, "rev-parse", "v6.2.0^{tree}"]).toString().trim();
  const upstream = execFileSync("git", ["-C", cac, "rev-parse", "v6.2.0^{commit}"]).toString().trim();
  // One parentless commit of upstream's tree (about 5 MB, not the 85 MB history), so the tree the runner downloads is
  // upstream's byte for byte; its message names the upstream commit.
  const commit = execFileSync("git", ["-C", cac, "-c", "user.name=bf70", "-c", "user.email=bf70@x", "commit-tree", tree,
    "-m", `configure-aws-credentials v6.2.0 (upstream ${upstream}), its tree only`],
  { env: { ...process.env, GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z", GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z" } }).toString().trim();
  // Pushed from a fresh repository holding only that commit: a push from a shallow clone is a shallow push, which the
  // install refuses ("shallow pushes are not supported", 400) even for a parentless commit.
  const bare = join(g.dir, "cac.git");
  execFileSync("git", ["init", "-q", "--bare", bare]);
  execFileSync("git", ["-C", cac, "push", "-q", bare, `${commit}:refs/heads/main`]);
  const push = g.run(["-C", bare, "push", "-q", "--force", g.url, `${commit}:refs/heads/main`, `${commit}:refs/tags/v6.2.0`], g.dir);
  log(`push v6.2.0 tree ${tree.slice(0, 12)} as ${commit.slice(0, 12)}: ${push.ok ? "ok" : push.out}`);
  const remote = g.remoteHead("refs/tags/v6.2.0");
  const repo = await api("GET", `/api/v3/repos/m3org/${name}`, null);
  result("action", { upstream, tree, commit, remote, anonymous_read: repo.status, visibility: repo.body?.visibility });
  log(`v6.2.0 on the install: ${remote === commit ? "same commit" : `MISMATCH ${remote}`}; anonymous read ${repo.status}`);
  if (remote !== commit || repo.status !== 200) process.exit(1);
} else if (phase === "unmirror") {
  // Teardown: the mirrored action's repository, `DELETE /repos/m3org/bf70-cac` (204), then an anonymous read 404s.
  const d = await api("DELETE", "/api/v3/repos/m3org/bf70-cac", admin);
  const after = await api("GET", "/api/v3/repos/m3org/bf70-cac", null);
  result("unmirror", { delete: d.status, anonymous_read_after: after.status });
  log(`m3org/bf70-cac: DELETE ${d.status}, anonymous read after ${after.status}`);
  if (![204, 404].includes(d.status) || after.status !== 404) process.exit(1);
} else if (phase === "job") {
  const runnerDir = a1;
  const fork = mode === "fork";
  const name = fork ? "bf70-fork" : "bf70";
  const R = `m3org/${name}`;
  // A dispatch's run key is (repository index, workflow): a label write moves the index, so each job is a new run.
  const lab = await api("POST", `/api/v3/repos/${R}/labels`, admin, { name: `bf70-${Date.now().toString(36)}`, color: "ededed" });
  log(`index bump (label): ${lab.status}`);
  const dir = join(out, `job-${tag}`);
  execFileSync("rm", ["-rf", join(runnerDir, "_diag"), join(runnerDir, "_work")]);
  const wf = new URL(fork ? "fork.yml" : "hosted.yml", import.meta.url).pathname;
  const env = { ...process.env, ...(fork ? { HOSTED_RUN_FORK: "1" } : {}) };
  const r = spawnSync("node", [new URL("../hosted-run.mjs", import.meta.url).pathname, runnerDir, wf, dir, name], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 20 * 60_000 });
  writeFileSync(join(dir, "driver.log"), (r.stdout || "") + (r.stderr || ""));
  const last = JSON.parse(readFileSync(join(dir, "run.jsonl"), "utf8").trim().split("\n").pop());
  const text = {};
  for (const job of last.jobs) {
    // The whole log is joined a few seconds after the job completes; until then `/logs` serves the blobs so far.
    for (let i = 0; i < 12; i++) {
      const x = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, { headers: { authorization: `token ${admin}` } });
      text[job.name] = await x.text();
      if (x.status === 200 && /Cleaning up orphan processes|Complete job/.test(text[job.name])) break;
      await sleep(5000);
    }
    writeFileSync(join(dir, `${job.name}.log`), text[job.name]);
  }
  const all = Object.values(text).join("\n");
  const has = (job, s) => (text[job] ?? "").includes(s);
  const concl = Object.fromEntries(last.jobs.map((j) => [j.name, j.s.Completed?.conclusion]));
  const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/.test(all);
  let checks;
  if (fork) {
    checks = {
      fork_succeeded: concl.fork === "Success",
      fork_no_url: has("fork", "fork, no URL"),
      fork_refused_403: /idtoken for a fork: 403/.test(text.fork ?? ""),
    };
  } else {
    const v = (text.sts ?? "").match(/verified kid=(\S+) published (\[[^\]]*\]) (\{.*\})/);
    checks = {
      sts_succeeded: concl.sts === "Success",
      denied_succeeded: concl.denied === "Success",
      exported_both: has("sts", "the runner exported both"),
      action_from_install: /Download action repository 'm3org\/bf70-cac@v6.2.0'/.test(text.sts ?? ""),
      s3_read_ok: has("sts", "s3 read ok"),
      verified_against_jwks: !!v,
      second_token_ok: has("sts", "second token ok"),
      denied_no_url: has("denied", "no id-token grant, no URL"),
      denied_refused_403: /idtoken without the grant: 403/.test(text.denied ?? ""),
    };
    if (v) checks.kid = v[1], checks.published = v[2], checks.claims = JSON.parse(v[3]);
  }
  checks.no_token_in_logs = !jwt;
  const ok = Object.entries(checks).every(([, x]) => x !== false);
  result(`job-${tag}`, { tag, fork, run: last, conclusions: concl, checks, ok, dir: dir.replace(`${process.cwd()}/`, "") });
  log(`job ${tag}: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
  if (!ok) process.exit(1);
} else {
  throw new Error("phase: action <cac checkout> <out> | job <runner dir> <out> <tag> [fork] | unmirror - <out>");
}
