// BF-74's token on graft-m4a (`tests/m4/wp3/bf74/row.sh token` and `teardown`; `source tests/am1/lf13/env-m4a.sh`,
// with GRAFT_EDGE on the custom hostname).
//   node tests/m4/wp3/bf74/token.mjs <token file>
//   node tests/m4/wp3/bf74/token.mjs --delete
// The first dispatches `token.yml` in m3org/$BF74_REPO (default `bf74`) on an unmodified runner (RUNNER, an unpacked
// actions/runner) through `../hosted-run.mjs`, whose job writes one ID token to <token file> on this host; it exits 1
// unless the job succeeded, the file holds a JWT, and no job log holds anything shaped like one. `--delete` deletes the
// repository at teardown and exits 1 unless an anonymous read then answers 404.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { EDGE, api, log, sleep, tokens } from "../../../am1/hosted/lib.mjs";

const name = process.env.BF74_REPO ?? "bf74";
const R = `m3org/${name}`;
const admin = tokens.admin;
const arg = process.argv[2];
const jwt = /eyJ[\w-]{10,}\.eyJ[\w-]{10,}\.[\w-]{20,}/;

if (arg === "--delete") {
  const d = await api("DELETE", `/api/v3/repos/${R}`, admin);
  const after = await api("GET", `/api/v3/repos/${R}`, null);
  log(`${R}: DELETE ${d.status}, anonymous read after ${after.status}`);
  if (![204, 404].includes(d.status) || after.status !== 404) process.exit(1);
} else if (arg) {
  const file = arg;
  const out = join(dirname(file), "run");
  rmSync(file, { force: true });
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const runnerDir = process.env.RUNNER;
  if (!runnerDir) throw new Error("RUNNER: an unpacked actions/runner");
  // The runner inherits this environment, so the job's step sees where to write the token.
  const env = { ...process.env, BF74_TOKEN_OUT: file };
  const wf = new URL("token.yml", import.meta.url).pathname;
  const r = spawnSync("node", [new URL("../hosted-run.mjs", import.meta.url).pathname, runnerDir, wf, out, name], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 20 * 60_000 });
  writeFileSync(join(out, "driver.log"), (r.stdout || "") + (r.stderr || ""));
  const last = JSON.parse(readFileSync(join(out, "run.jsonl"), "utf8").trim().split("\n").pop());
  let text = "";
  for (const job of last.jobs) {
    for (let i = 0; i < 12; i++) {
      const x = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, { headers: { authorization: `token ${admin}` } });
      text = await x.text();
      if (x.status === 200 && /Cleaning up orphan processes|Complete job/.test(text)) break;
      await sleep(5000);
    }
  }
  const said = text.split("\n").find((l) => l.includes("token written,") && !l.includes("##[group]")) ?? "";
  const checks = {
    succeeded: last.jobs.every((j) => j.s.Completed?.conclusion === "Success"),
    file_is_jwt: existsSync(file) && jwt.test(readFileSync(file, "utf8")),
    iss_from_job: said.replace(/.*iss /, "").trim() || null,
    no_token_in_logs: !jwt.test(text),
  };
  writeFileSync(join(out, "token.json"), JSON.stringify({ run: last, checks }, null, 2));
  log(`token: ${JSON.stringify(checks)}`);
  if (!checks.succeeded || !checks.file_is_jwt || !checks.no_token_in_logs) process.exit(1);
} else {
  throw new Error("usage: token.mjs <token file> | --delete");
}
