// BF-73's install half on graft-m4a (`tests/m4/wp3/bf73/row.sh`; `source tests/am1/lf13/env-m4a.sh`).
//   node tests/m4/wp3/bf73/claims.mjs <pr|dispatch|custom> <out dir>
//   node tests/m4/wp3/bf73/claims.mjs delete <out dir>
// `pr` starts `claims.yml` in m3org/$BF73_REPO from a same-repository pull request, `dispatch` by `workflow_dispatch`
// on `main`, and `custom` first puts the repository's `sub` customization (`PUT .../actions/oidc/customization/sub`,
// read back with GET) and then dispatches again. Each runs on an unmodified runner (RUNNER) through `../hosted-run.mjs`;
// the job writes its ID token to a file on this host. The expected claims come from the install's own records, not the
// token: the admin's `GET /user` (actor, actor_id), the repository's run list (run_number: this run's place among the
// workflow's runs), and the pull request's branch (head_ref). `tests/m4/wp3/bf74/rp.py` then checks the token against
// the trust policy naming them (accepted), and against one naming another actor and one naming another branch (each
// verified but refused). Writes `<out>/<phase>.json`; exits 1 when a check fails. No token is printed.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, api, log, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const [phase, out] = process.argv.slice(2);
const name = process.env.BF73_REPO ?? "bf73";
const R = `m3org/${name}`;
const WF = ".github/workflows/claims.yml";
// The customization the third run's `sub` follows: GHES's own example keys plus two of BF-73's claims.
const KEYS = ["repo", "context", "actor", "run_number"];
const admin = tokens.admin;
const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/;
mkdirSync(out, { recursive: true });
const result = (k, v) => writeFileSync(join(out, `${k}.json`), JSON.stringify(v, null, 2));
const SUB = `/api/v3/repos/${R}/actions/oidc/customization/sub`;

if (phase === "delete") {
  const d = await api("DELETE", `/api/v3/repos/${R}`, admin);
  const after = await api("GET", `/api/v3/repos/${R}`, null);
  result("delete", { delete: d.status, anonymous_read_after: after.status });
  log(`${R}: DELETE ${d.status}, anonymous read after ${after.status}`);
  if (![204, 404].includes(d.status) || after.status !== 404) process.exit(1);
  process.exit(0);
}
if (!["pr", "dispatch", "custom"].includes(phase)) throw new Error("phase: pr | dispatch | custom | delete");

const checks = {};
if (phase === "custom") {
  const put = await api("PUT", SUB, admin, { use_default: false, include_claim_keys: KEYS });
  const got = await api("GET", SUB, admin);
  checks.put_201 = put.status === 201;
  checks.read_back = got.status === 200 && got.body?.use_default === false && JSON.stringify(got.body?.include_claim_keys) === JSON.stringify(KEYS);
  log(`PUT sub ${put.status}; GET ${got.status} ${JSON.stringify(got.body)}`);
}

// The run, on a fresh runner; the job writes the token here.
const dir = join(out, phase);
const file = join(dir, "token.jwt");
rmSync(dir, { recursive: true, force: true });
mkdirSync(dir, { recursive: true, mode: 0o700 });
const runnerDir = process.env.RUNNER;
if (!runnerDir) throw new Error("RUNNER: an unpacked actions/runner");
rmSync(join(runnerDir, "_diag"), { recursive: true, force: true });
rmSync(join(runnerDir, "_work"), { recursive: true, force: true });
const env = { ...process.env, BF73_TOKEN_OUT: file, ...(phase === "pr" ? { HOSTED_RUN_PR: "1" } : {}) };
const wf = new URL("claims.yml", import.meta.url).pathname;
const r = spawnSync("node", [new URL("../hosted-run.mjs", import.meta.url).pathname, runnerDir, wf, dir, name], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 20 * 60_000 });
const driver = (r.stdout || "") + (r.stderr || "");
writeFileSync(join(dir, "driver.log"), driver);
const last = JSON.parse(readFileSync(join(dir, "run.jsonl"), "utf8").trim().split("\n").pop());
const job = last.jobs[0];
let text = "";
for (let i = 0; i < 12; i++) {
  const x = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, { headers: { authorization: `token ${admin}` } });
  text = await x.text();
  if (x.status === 200 && /Cleaning up orphan processes|Complete job/.test(text)) break;
  await sleep(5000);
}
writeFileSync(join(dir, "claims.log"), text);
checks.job_succeeded = job.s.Completed?.conclusion === "Success";
checks.token_file = existsSync(file) && jwt.test(readFileSync(file, "utf8"));
checks.no_token_in_logs = !jwt.test(text) && !jwt.test(driver);

// What the install says the claims must be.
const me = (await api("GET", "/api/v3/user", admin)).body;
const runs = (await api("GET", `/api/graftyard/v1/repos/${R}/actions/runs`, admin)).body?.runs ?? [];
const runId = Number(driver.match(/run (\d+) created/)?.[1]);
const mine = runs.filter((x) => x.workflow === WF).map((x) => Number(x.id)).sort((a, b) => a - b);
const number = mine.filter((id) => id <= runId).length;
const pr = driver.match(/pull request #(\d+) from (\S+) into main/);
const ref = pr ? `refs/pull/${pr[1]}/merge` : "refs/heads/main";
const context = pr ? "pull_request" : `ref:${ref}`;
const legacy = `repo:${R}:${context}`;
const want = {
  sub: phase === "custom" ? `repo:${R}:${context}:actor:${me.login}:run_number:${number}` : legacy,
  actor: me.login,
  actor_id: String(me.id),
  head_ref: pr ? pr[2] : "",
  base_ref: pr ? "main" : "",
  run_number: String(number),
  job_workflow_ref: `${R}/${WF}@${ref}`,
  event_name: pr ? "pull_request" : "workflow_dispatch",
};
checks.context_line = new RegExp(`context actor=${me.login} run_number=${number} event=${want.event_name}`).test(text);

// The trust policies, as a relying party with stock urllib reads them. A workers.dev issuer refuses urllib's agent
// (F130; BF-74 removes that on a custom host), so only there the relying party names its own agent.
const iss = JSON.parse(Buffer.from(readFileSync(file, "utf8").split(".")[1], "base64url")).iss;
const agent = new URL(iss).hostname.endsWith(".workers.dev") ? ["--agent", "bf73-rp"] : [];
const rp = (policy) => {
  const a = Object.entries(policy).flatMap(([k, v]) => ["--want", `${k}=${v}`]);
  const x = spawnSync("python3", [new URL("../bf74/rp.py", import.meta.url).pathname, file, "--aud", "bf73", ...agent, ...a], { encoding: "utf8" });
  return { exit: x.status, ...JSON.parse(x.stdout.trim().split("\n").pop()) };
};
const matching = rp(want);
const otherActor = rp({ ...want, actor: `${me.login}-not`, actor_id: String(me.id + 1) });
const otherBranch = rp(pr ? { ...want, head_ref: `${pr[2]}-not` } : { ...want, sub: want.sub.replace("refs/heads/main", "refs/heads/other") });
checks.policy_accepts = matching.exit === 0 && matching.accepted === true;
checks.other_actor_refused = otherActor.exit === 1 && otherActor.verified === true && !!otherActor.mismatch.actor;
checks.other_branch_refused = otherBranch.exit === 1 && otherBranch.verified === true && Object.keys(otherBranch.mismatch).length === 1;
// The run is in the workflow's list, so its place in it is defined (run ids grow with creation [inference]).
checks.run_listed = mine.includes(runId);
const ok = Object.values(checks).every((x) => x === true);
result(phase, { phase, run: runId, number, want, claims: matching.claims, mismatch: matching.mismatch, fetch: matching.fetch, checks, ok });
log(`${phase}: ${ok ? "PASS" : "FAIL"} ${JSON.stringify(checks)} mismatch ${JSON.stringify(matching.mismatch)}`);
rmSync(file, { force: true });
if (!ok) process.exit(1);
