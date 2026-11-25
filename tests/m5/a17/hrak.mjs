// The HostedRunnersAppKey rotation row's install half (`tests/m5/a17/hrak.sh`): App 1835008's record on graft-m4a,
// an independent installation-token mint, and the hosted jobs the switch is measured under. Environment as
// `tests/am1/hosted/lib.mjs` (graft-m4a's env), plus APPKEYS (the `appkeys` binary). No secret goes on argv, in a URL
// or in a log line (`scan`); a PEM is read from a 0600 file.
//
//   node tests/m5/a17/hrak.mjs show                 the App's keys (ids, created, SPKI SHA-256 prefixes) and webhook
//   node tests/m5/a17/hrak.mjs add <id> <spki.der>  PutApp with that key added beside the current ones
//   node tests/m5/a17/hrak.mjs only <id>            PutApp with only that key
//   node tests/m5/a17/hrak.mjs mint <pem>           App JWT under <pem>, then access_tokens on installation 7190002
//                                                   with graft-runners' permissions: prints the status only
//   node tests/m5/a17/hrak.mjs repo                 creates m3org/<HRAK_REPO> with hosted/smoke.yml on main
//   node tests/m5/a17/hrak.mjs jobs <out> <stop>    every JOB_EVERY_S (240) one smoke.yml run until <stop> exists,
//                                                   then waits for every run; one jobs.jsonl row per change
//   node tests/m5/a17/hrak.mjs unrepo               deletes the repository
import { spawnSync } from "node:child_process";
import { createSign } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, commit, log, member, refusal, row, secret, sleep, tokens, voterNames } from "../../am1/hosted/lib.mjs";

const APP = 1835008, INSTALLATION = 7190002, CHAIN = "authz:7100003";
const NAME = process.env.HRAK_REPO ?? "hrak-jobs", R = `m3org/${NAME}`, A = `/api/graftyard/v1/repos/${R}/actions`;
const WF = ".github/workflows/smoke.yml";
const [cmd, a1, a2] = process.argv.slice(2);
const admin = tokens.admin;

async function fold() {
  const names = await voterNames(CHAIN);
  for (const s of [0, 1, 2]) {
    try {
      const v = (await member(CHAIN, s, "/read", { q: { dep: true } }, 30_000, names[s]))?.v?.value;
      if (typeof v === "string" && v.length) return v;
    } catch { /* next site */ }
  }
  throw new Error(`${CHAIN}: no member answered the dep read`);
}
// `appkeys` (tests/m3/hosted/tools) decodes the fold and prints the App's keys, or the PutApp that changes them.
async function run(...args) {
  const r = spawnSync(process.env.APPKEYS, [String(APP), ...args], { input: await fold(), encoding: "utf8" });
  if (r.status !== 0) throw new Error(`appkeys ${args[0]}: ${r.stderr.slice(-300)}`);
  return r.stdout.trim();
}

if (cmd === "show") {
  console.log(await run("show"));
} else if (cmd === "add" || cmd === "only") {
  const op = JSON.parse(await run(...(cmd === "add" ? ["add", a1, a2] : ["only", a1])));
  const o = await commit(CHAIN, { cred: { account: null, key: null }, op });
  console.log(JSON.stringify({ chain: CHAIN, op: `PutApp (${cmd} ${a1})`, outcome: refusal(o) }));
  console.log(await run("show"));
  if (!o?.ok) process.exit(1);
} else if (cmd === "mint") {
  const b = (x) => Buffer.from(x).toString("base64url"), now = Math.floor(Date.now() / 1000);
  // graft-runners' claims (`workers/runners/src/worker.rs`, `app_jwt`): a 10-minute window from a minute back.
  const h = b(JSON.stringify({ alg: "RS256", typ: "JWT" })), p = b(JSON.stringify({ iat: now - 60, exp: now + 540, iss: APP }));
  const jwt = secret(`${h}.${p}.${b(createSign("RSA-SHA256").update(`${h}.${p}`).sign(readFileSync(a1, "utf8")))}`);
  const r = await api("POST", `/api/v3/app/installations/${INSTALLATION}/access_tokens`, null,
    { permissions: { organization_self_hosted_runners: "write" } }, { authorization: `Bearer ${jwt}` });
  if (r.body?.token) secret(r.body.token);
  console.log(JSON.stringify({ access_tokens: r.status, date: r.headers.date ?? null, message: r.status === 201 ? null : JSON.stringify(r.body).slice(0, 200) }));
  if (r.status !== 201) process.exit(1);
} else if (cmd === "repo") {
  const made = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name: NAME, private: true, auto_init: true });
  if (![201, 422].includes(made.status)) throw new Error(`create ${made.status}`);
  const g = new Git(R).as(admin);
  for (let i = 0; !g.clone().ok; i++) { if (i > 10) throw new Error("clone"); await sleep(3000); }
  mkdirSync(join(g.wt, ".github/workflows"), { recursive: true });
  writeFileSync(join(g.wt, WF), readFileSync(new URL("../../m4/wp4/hosted/smoke.yml", import.meta.url)));
  g.run(["add", "-A"]);
  if (g.run(["commit", "-q", "-m", `Add ${WF}`]).ok && !g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]).ok) throw new Error("push");
  log(`repo ${R}: ${made.status}, ${WF} on main`);
} else if (cmd === "jobs") {
  const rows = join(a1, "jobs.jsonl"), every = Number(process.env.JOB_EVERY_S ?? 240) * 1000;
  const runs = new Map(); // run -> {dispatched, date, state, conclusion}
  let next = 0;
  for (;;) {
    const stop = existsSync(a2), now = Date.now();
    if (!stop && now >= next) {
      next = now + every;
      const r = await api("POST", `${A}/dispatch`, admin, { workflow: WF, ref: "main" });
      const id = r.status === 201 ? r.body.run : null;
      row(rows, { t: now, date: r.headers.date ?? null, dispatch: r.status, run: id });
      if (id) runs.set(id, { dispatched: now, date: r.headers.date, state: null });
    }
    for (const [id, e] of runs) {
      if (e.conclusion) continue;
      const r = await api("GET", `${A}/runs/${id}`, admin);
      if (r.status !== 200) continue;
      const j = r.body.value.jobs[0];
      const st = j ? Object.keys(j.state)[0] : "none";
      if (st !== e.state) {
        e.state = st;
        if (st === "Completed") e.conclusion = j.state.Completed.conclusion;
        row(rows, { t: Date.now(), date: r.headers.date ?? null, run: id, state: st, conclusion: e.conclusion ?? null });
      }
    }
    const open = [...runs.values()].filter((e) => !e.conclusion);
    if (stop && (!open.length || Date.now() - Math.max(...open.map((e) => e.dispatched)) > 30 * 60_000)) break;
    await sleep(10_000);
  }
  const by = {};
  for (const e of runs.values()) by[e.conclusion ?? "unfinished"] = (by[e.conclusion ?? "unfinished"] ?? 0) + 1;
  row(rows, { t: Date.now(), done: true, runs: runs.size, by });
  log(`jobs: ${runs.size} runs ${JSON.stringify(by)}`);
} else if (cmd === "unrepo") {
  const r = await api("DELETE", `/api/v3/repos/${R}`, admin);
  console.log(JSON.stringify({ delete: R, status: r.status }));
} else {
  console.log("show | add <id> <spki.der> | only <id> | mint <pem> | repo | jobs <out> <stop> | unrepo");
  process.exit(2);
}
