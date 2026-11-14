// BF-59 on a hosted install: `gh secret set` for a repository, an environment, and an org, then a job reading all
// three; then rotate the `SealKey` and the wrapping key; a fork-PR job sees none; no plaintext in any entry or log.
//   source tests/am1/lf13/env-m4a.sh
//   node tests/m4/wp3/bf59.mjs <runner dir> <out dir> <phase>
// Phases, each a separate call so a site or edge day can sit between them:
//   set       generates three values (kept only in <out>/values.json, mode 600), sets them with `gh secret set`
//             (repository, `--env bf59env`, `--org m3org --visibility selected --repos <repo>`) and a variable with
//             `gh variable set`, and checks the list routes show the names and no value.
//   job [tag] dispatches bf59.yml on an unmodified runner (`hosted-run.mjs`) and checks the job's log: each step
//             prints the value's SHA-256 prefix and length, which must match; `fork` dispatches it as a fork PR, whose
//             log must show the empty value's hash for every secret, and the variable.
//   rotate    `POST /api/graftyard/v1/orgs/m3org/actions/rotate-seal-key`.
//   rewrap    `POST .../rewrap-secrets` (after graft-ops was given the new wrapping key slot as current).
//   scan      reads every committed entry of the repository chain and the org's `Authz` chain from each member
//             (`/entries`), every job log under the run directories, and the runner diagnostics, and fails on any
//             value, its base64, or its hex.
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, HOST, api, log, member, secret, sleep, tokens, voterNames } from "../../am1/hosted/lib.mjs";

const [runnerDir, out, phase, tag] = process.argv.slice(2);
const NAME = "wp3-bf59";
const R = `m3org/${NAME}`;
mkdirSync(out, { recursive: true });
const vfile = join(out, "values.json");
const admin = tokens.admin;
const hash = (s) => createHash("sha256").update(s).digest("hex").slice(0, 16);
const gh = (args, input) => {
  const r = spawnSync("gh", args, {
    input,
    encoding: "utf8",
    env: { ...process.env, GH_HOST: HOST, GH_ENTERPRISE_TOKEN: admin, GH_CONFIG_DIR: join(out, "ghcfg"), GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" },
  });
  log(`gh ${args.join(" ")} -> ${r.status} ${(r.stderr || "").trim().slice(0, 200)}`);
  if (r.status !== 0) throw new Error(`gh ${args[0]} ${args[1]} failed`);
  return r.stdout;
};
const result = (k, v) => writeFileSync(join(out, `${k}.json`), JSON.stringify(v, null, 2));

if (phase === "set") {
  const rnd = () => `bf59-${createHash("sha256").update(String(Math.random()) + Date.now()).digest("base64url").slice(0, 24)}`;
  const values = existsSync(vfile) ? JSON.parse(readFileSync(vfile, "utf8")) : { repo: rnd(), env: rnd(), org: rnd(), var: "bf59-plain-variable" };
  writeFileSync(vfile, JSON.stringify(values), { mode: 0o600 });
  for (const v of [values.repo, values.env, values.org]) secret(v);
  // The repository exists once `hosted-run.mjs` created it; make it here first so the secrets precede the job.
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
  log(`repository ${R}: ${made.status}`);
  const repoId = (await api("GET", `/api/v3/repos/${R}`, admin)).body.id;
  gh(["secret", "set", "BF59_REPO", "-R", `${HOST}/${R}`], values.repo);
  gh(["secret", "set", "BF59_ENV", "-R", `${HOST}/${R}`, "--env", "bf59env"], values.env);
  gh(["secret", "set", "BF59_ORG", "--org", "m3org", "--visibility", "selected", "--repos", NAME], values.org);
  gh(["variable", "set", "BF59_VAR", "-R", `${HOST}/${R}`, "--body", values.var]);
  const lists = {
    repo: (await api("GET", `/api/v3/repos/${R}/actions/secrets`, admin)).body,
    env: (await api("GET", `/api/v3/repos/${R}/environments/bf59env/secrets`, admin)).body,
    org: (await api("GET", `/api/v3/orgs/m3org/actions/secrets`, admin)).body,
    key: (await api("GET", `/api/v3/orgs/m3org/actions/secrets/public-key`, admin)).body,
    vars: (await api("GET", `/api/v3/repos/${R}/actions/variables`, admin)).body,
    repoId,
  };
  const text = JSON.stringify(lists);
  const leaked = [values.repo, values.env, values.org].filter((v) => text.includes(v));
  result("set", { lists, leaked: leaked.length });
  log(`set: key ${lists.key?.key_id}; names ${[lists.repo, lists.env, lists.org].map((l) => l?.secrets?.map((s) => s.name).join(",")).join(" | ")}; values in lists: ${leaked.length}`);
  if (leaked.length) process.exit(1);
} else if (phase === "job") {
  const values = JSON.parse(readFileSync(vfile, "utf8"));
  for (const v of [values.repo, values.env, values.org]) secret(v);
  const fork = tag === "fork";
  // A dispatch's run key is (repository index, workflow): a label write first moves the index, so each job phase is a
  // new run rather than a replay of the last one.
  const lab = await api("POST", `/api/v3/repos/${R}/labels`, admin, { name: `bf59-${Date.now().toString(36)}`, color: "ededed" });
  log(`index bump (label): ${lab.status}`);
  const dir = join(out, `job-${tag ?? "plain"}-${Date.now().toString(36)}`);
  execFileSync("rm", ["-rf", join(runnerDir, "_diag"), join(runnerDir, "_work")]);
  const env = { ...process.env, ...(fork ? { HOSTED_RUN_FORK: "1" } : {}) };
  const r = spawnSync("node", [new URL("hosted-run.mjs", import.meta.url).pathname, runnerDir, new URL("hosted/bf59.yml", import.meta.url).pathname, dir, NAME], { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"], timeout: 15 * 60_000 });
  writeFileSync(join(dir, "driver.log"), (r.stdout || "") + (r.stderr || ""));
  const last = readFileSync(join(dir, "run.jsonl"), "utf8").trim().split("\n").pop();
  const job = JSON.parse(last).jobs[0];
  const logs = await api("GET", `/api/v3/repos/${R}/actions/jobs/${job.id}/logs`, admin);
  const text = typeof logs.body === "string" ? logs.body : JSON.stringify(logs.body);
  writeFileSync(join(dir, "job.log"), text);
  const want = fork
    ? { repo: [hash(""), 0], env: [hash(""), 0], org: [hash(""), 0] }
    : { repo: [hash(values.repo), values.repo.length], env: [hash(values.env), values.env.length], org: [hash(values.org), values.org.length] };
  const seen = {};
  for (const k of ["repo", "env", "org"]) {
    const m = text.match(new RegExp(`\\b${k} ([0-9a-f]{16}) (\\d+)`));
    seen[k] = m ? [m[1], Number(m[2])] : null;
  }
  const varOk = text.includes(`var ${values.var}`);
  const ok = ["repo", "env", "org"].every((k) => JSON.stringify(seen[k]) === JSON.stringify(want[k])) && varOk && job.s.Completed?.conclusion === "Success";
  result(`job-${tag ?? "plain"}`, { job: job.id, conclusion: job.s.Completed?.conclusion, fork, want, seen, varOk, ok, logStatus: logs.status, dir });
  log(`job ${job.id} (${tag ?? "plain"}): ${ok ? "PASS" : "FAIL"} seen ${JSON.stringify(seen)} want ${JSON.stringify(want)} var ${varOk}`);
  if (!ok) process.exit(1);
} else if (phase === "rotate" || phase === "rewrap") {
  const op = phase === "rotate" ? "rotate-seal-key" : "rewrap-secrets";
  const r = await api("POST", `/api/graftyard/v1/orgs/m3org/actions/${op}`, admin, {});
  result(phase, r);
  log(`${op}: ${r.status} ${JSON.stringify(r.body)}`);
  if (r.status !== 200) process.exit(1);
  const key = (await api("GET", `/api/v3/orgs/m3org/actions/secrets/public-key`, admin)).body;
  log(`public key now ${key.key_id}`);
} else if (phase === "scan") {
  const values = JSON.parse(readFileSync(vfile, "utf8"));
  const needles = [];
  for (const v of [values.repo, values.env, values.org]) {
    needles.push(v, Buffer.from(v).toString("base64").replace(/=+$/, ""), Buffer.from(v).toString("hex"));
  }
  const hits = [];
  const check = (where, s) => { for (const n of needles) if (s.includes(n)) hits.push(where); };
  const repoId = JSON.parse(readFileSync(join(out, "set.json"), "utf8")).lists.repoId;
  let entries = 0;
  for (const chain of [`repo:${repoId}`, "authz:7100001"]) {
    const names = await voterNames(chain);
    for (const site of [0, 1, 2]) {
      for (let from = 1; ; ) {
        const r = await member(chain, site, `/entries?from=${from}&local=1`, undefined, 60_000, names[site]);
        const es = r.v?.entries ?? [];
        if (!es.length) break;
        const s = JSON.stringify(es);
        check(`${chain} site ${site} from ${from}`, s);
        entries += es.length;
        const last = es[es.length - 1];
        const idx = last.index ?? last[0]?.index ?? from + es.length - 1;
        if (idx < from) break;
        from = idx + 1;
      }
    }
  }
  let files = 0;
  const walk = (d) => { for (const f of readdirSync(d)) { const p = join(d, f); const st = statSync(p); if (st.isDirectory()) walk(p); else if (st.size < 50e6 && !p.endsWith("values.json")) { files += 1; check(p, readFileSync(p, "latin1")); } } };
  walk(out);
  if (existsSync(join(runnerDir, "_diag"))) walk(join(runnerDir, "_diag"));
  result("scan", { entries, files, hits });
  log(`scan: ${entries} entries, ${files} files, ${hits.length} hits ${JSON.stringify(hits.slice(0, 5))}`);
  if (hits.length) process.exit(1);
} else {
  throw new Error("phase: set | job [fork] | rotate | rewrap | scan");
}
