// BF-70's install half on graft-m4a (`tests/m4/wp3/oidc/hosted-row.sh` calls it; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/oidc/hosted.mjs action <configure-aws-credentials checkout> <out dir>
//   node tests/m4/wp3/oidc/hosted.mjs job <runner dir> <out dir> <tag> [fork]
//   node tests/m4/wp3/oidc/hosted.mjs precheck <runner dir> <out dir>
//   node tests/m4/wp3/oidc/hosted.mjs unmirror - <out dir>
// `action` creates the public repository `m3org/bf70-cac` and pushes configure-aws-credentials v6.2.0's tree to it as
// `main` and tag `v6.2.0` (the install resolves `uses:` only in its own repositories, F90), and records the tree hash,
// which must equal upstream's; `unmirror` deletes it at teardown. `job` dispatches `hosted.yml` (or, with `fork`,
// `fork.yml` as a fork pull request's run) on an unmodified runner through `../hosted-run.mjs`, `hosted.yml` from a
// same-repository pull request (`HOSTED_RUN_PR=1`; F127), reads every job's
// log, and writes `<out>/job-<tag>.json` with what each check saw. It exits 1 when a check fails. It never prints a
// token: the workflow prints only the kid, the published kids and the claims, and the log scan fails on anything
// shaped like a JWT.
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, api, log, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [phase, a1, out, tag, mode] = process.argv.slice(2);
// The row's repositories: a deleted name stays taken until its `retain_until` (90 days), so each row tag takes its own
// (`hosted-row.sh` sets BF70_REPO=bf70-<tag>; r1 ran as `bf70`).
const base = process.env.BF70_REPO ?? "bf70";
const cacName = `${base}-cac`;
const admin = tokens.admin;
mkdirSync(out, { recursive: true });
const result = (k, v) => writeFileSync(join(out, `${k}.json`), JSON.stringify(v, null, 2));

if (phase === "action") {
  const cac = a1;
  const name = cacName;
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
  const d = await api("DELETE", `/api/v3/repos/m3org/${cacName}`, admin);
  const after = await api("GET", `/api/v3/repos/m3org/${cacName}`, null);
  result("unmirror", { delete: d.status, anonymous_read_after: after.status });
  log(`m3org/${cacName}: DELETE ${d.status}, anonymous read after ${after.status}`);
  if (![204, 404].includes(d.status) || after.status !== 404) process.exit(1);
} else if (phase === "job" || phase === "precheck") {
  const runnerDir = a1;
  const pre = phase === "precheck";
  const fork = mode === "fork";
  const name = fork ? `${base}-fork` : base;
  const R = `m3org/${name}`;
  if (fork) {
    // A dispatch's run key is (repository index, workflow): a label write moves the index, so each job is a new run.
    const lab = await api("POST", `/api/v3/repos/${R}/labels`, admin, { name: `bf70-${Date.now().toString(36)}`, color: "ededed" });
    log(`index bump (label): ${lab.status}`);
  }
  const dir = join(out, pre ? "precheck" : `job-${tag}`);
  execFileSync("rm", ["-rf", join(runnerDir, "_diag"), join(runnerDir, "_work")]);
  let wf = new URL(fork ? "fork.yml" : pre ? "precheck.yml" : "hosted.yml", import.meta.url).pathname;
  if (!fork && !pre) {
    // `uses:` names the mirrored action's repository, this row's.
    mkdirSync(join(out, "wf"), { recursive: true });
    const y = readFileSync(wf, "utf8").replace("uses: m3org/bf70-cac@v6.2.0", `uses: m3org/${cacName}@v6.2.0`);
    writeFileSync((wf = join(out, "wf", "hosted.yml")), y);
  }
  // The fork-PR job stays a dispatch marked fork (no fork route exists); every other job is a pull request's run.
  const env = { ...process.env, ...(fork ? { HOSTED_RUN_FORK: "1" } : { HOSTED_RUN_PR: "1" }) };
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
  // Output lines only: the runner also echoes each step's script (`##[group]Run` and its cyan `[36;1m` lines), which
  // hold the same strings (r2's first `job before` read "second token ok" from the echo of a step that failed first).
  const said = (job) => (text[job] ?? "").split("\n").filter((l) => !l.includes("##[group]") && !l.includes("\u001b[36;1m")).join("\n");
  const has = (job, s) => said(job).includes(s);
  const concl = Object.fromEntries(last.jobs.map((j) => [j.name, j.s.Completed?.conclusion]));
  const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/.test(all);
  let checks;
  if (pre) {
    // The lead's precheck: only whether the URL is set; abort the row unless it is.
    checks = { precheck_succeeded: concl.precheck === "Success", url_set: has("precheck", "ACTIONS_ID_TOKEN_REQUEST_URL set: yes") };
  } else if (fork) {
    checks = {
      fork_succeeded: concl.fork === "Success",
      fork_no_url: has("fork", "fork, no URL"),
      // `may_mint` refuses a fork before it reads the grant, so the 403 names the fork, not a missing grant.
      fork_refused_403: /idtoken for a fork: 403 .*fork/i.test(said("fork")),
    };
  } else {
    const v = said("sts").match(/verified kid=(\S+) published (\[[^\]]*\]) (\{.*\})/);
    checks = {
      sts_succeeded: concl.sts === "Success",
      denied_succeeded: concl.denied === "Success",
      exported_both: has("sts", "the runner exported both"),
      action_from_install: (text.sts ?? "").includes(`Download action repository 'm3org/${cacName}@v6.2.0'`),
      s3_read_ok: has("sts", "s3 read ok"),
      verified_against_jwks: !!v,
      second_token_ok: has("sts", "second token ok"),
      denied_no_url: has("denied", "no id-token grant, no URL"),
      denied_refused_403: /idtoken without the grant: 403/.test(said("denied")),
    };
    if (v) {
      const c = JSON.parse(v[3]);
      checks.kid = v[1], checks.published = v[2], checks.claims = c;
      // The lead's ruling for r2: a same-repository pull request's token.
      checks.sub_is_pull_request = c.sub === `repo:m3org/${base}:pull_request`;
      checks.event_name_pull_request = c.event_name === "pull_request";
      // F121's known gap, recorded and not a pass line: GitHub sets base_ref on pull_request, this install leaves it
      // empty.
      checks.gap_f121_base_ref = c.base_ref_set === false ? "empty (F121)" : `set: ${c.base_ref_set}`;
    }
  }
  checks.no_token_in_logs = !jwt;
  const ok = Object.entries(checks).every(([, x]) => x !== false);
  result(pre ? "precheck" : `job-${tag}`, { tag, fork, run: last, conclusions: concl, checks, ok, dir: dir.replace(`${process.cwd()}/`, "") });
  log(`${pre ? "precheck" : `job ${tag}`}: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)}`);
  if (!ok) process.exit(1);
} else {
  throw new Error("phase: action <cac checkout> <out> | precheck <runner dir> <out> | job <runner dir> <out> <tag> [fork] | unmirror - <out>");
}
