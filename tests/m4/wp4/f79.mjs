// F79's hosted check on graft-m4a (decisions/OPEN-QUESTIONS.md, F79): after the edge fix, a hosted job still acquires
// with its repository's name and its secrets. Sets a repository secret with `gh secret set` (the value is kept only in
// <out>/value.json, mode 600), dispatches hosted/f79.yml (5 `ubuntu-latest` legs), and reads each job's log: every leg
// must print `repo m3org/wp4-f79` and the secret's SHA-256 prefix and length, and end `Success`. graft-runners starts
// the Sandboxes from the App's `workflow_job` delivery, as in BF-60.
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp4/f79.mjs <out dir>
import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { EDGE, Git, HOST, api, log, secret, sleep, tokens } from "../../am1/hosted/lib.mjs";

const out = resolve(process.argv[2] ?? "tests/m4/raw/wp4/f79");
mkdirSync(out, { recursive: true });
const NAME = "wp4-f79";
const R = `m3org/${NAME}`;
const admin = tokens.admin;
const repoRoot = new URL("../../..", import.meta.url).pathname;
const hash = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);
const vfile = join(out, "value.json");
const value = existsSync(vfile) ? JSON.parse(readFileSync(vfile, "utf8")).value : `f79-${randomBytes(18).toString("base64url")}`;
writeFileSync(vfile, JSON.stringify({ value }), { mode: 0o600 });
secret(value);

const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
log(`repository ${R}: ${made.status}`);
const g = new Git(R).as(admin);
for (let i = 0; !g.clone().ok; i++) {
  if (i > 10) throw new Error("clone");
  await sleep(3000);
}
const set = spawnSync("gh", ["secret", "set", "F79_SECRET", "-R", `${HOST}/${R}`], {
  input: value, encoding: "utf8",
  env: { ...process.env, GH_HOST: HOST, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" },
});
log(`gh secret set F79_SECRET -> ${set.status} ${(set.stderr || "").trim().slice(0, 200)}`);
if (set.status !== 0) process.exit(1);
const wf = ".github/workflows/f79.yml";
mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
writeFileSync(join(g.wt, wf), readFileSync(join(repoRoot, "tests/m4/wp4/hosted/f79.yml")));
g.run(["add", "-A"]);
if (g.run(["commit", "-q", "-m", `Add ${wf}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push main");

const A = `/api/graftyard/v1/repos/${R}/actions`;
const d = await api("POST", `${A}/dispatch`, admin, { workflow: wf, ref: "main" });
if (d.status !== 201) throw new Error(`dispatch: ${d.status} ${JSON.stringify(d.body).slice(0, 300)}`);
const run = d.body.run;
const t0 = Date.now();
log(`run ${run} dispatched`);
let jobs = [];
for (;;) {
  const r = await api("GET", `${A}/runs/${run}`, admin);
  if (r.status === 200) jobs = r.body.value.jobs;
  const done = jobs.filter((j) => j.state.Completed).length;
  log(`+${Math.round((Date.now() - t0) / 1000)}s ${done}/${jobs.length} done`);
  if (jobs.length === 5 && done === 5) break;
  if (Date.now() - t0 > 20 * 60_000) { log("gave up after 20 minutes"); break; }
  await sleep(10_000);
}
const want = [hash(value), value.length];
const legs = [];
for (const j of jobs) {
  let text = "";
  for (let i = 0; i < 12; i++) {
    const r = await fetch(`${EDGE}/api/v3/repos/${R}/actions/jobs/${j.spec.id}/logs`, { headers: { authorization: `token ${admin}` } });
    text = await r.text();
    if (/Z secret [0-9a-f]{16} \d+\s*$/m.test(text)) break;
    await sleep(5000);
  }
  writeFileSync(join(out, `job-${j.spec.id}.log`), text);
  // Output lines only: the step's echoed script also holds "repo %s" and "secret %s %s" (run 2097152's first read).
  const repo = text.match(/Z repo (\S+)\s*$/m)?.[1] ?? null;
  const m = text.match(/Z secret ([0-9a-f]{16}) (\d+)\s*$/m);
  const seen = m ? [m[1], Number(m[2])] : null;
  const conclusion = j.state.Completed?.conclusion ?? Object.keys(j.state)[0];
  const ok = repo === R && JSON.stringify(seen) === JSON.stringify(want) && conclusion === "Success";
  legs.push({ job: j.spec.id, name: j.spec.name, conclusion, repo, seen, ok });
  log(`job ${j.spec.id} ${j.spec.name}: ${ok ? "PASS" : "FAIL"} ${conclusion} repo ${repo} secret ${JSON.stringify(seen)}`);
}
const pass = legs.length === 5 && legs.every((l) => l.ok);
writeFileSync(join(out, "result.json"), JSON.stringify({ repo: R, run, want, legs, pass, seconds: Math.round((Date.now() - t0) / 1000) }, null, 2));
log(`F79 check: ${pass ? "PASS" : "FAIL"} (${legs.filter((l) => l.ok).length} of ${legs.length} legs)`);
process.exit(pass ? 0 : 1);
