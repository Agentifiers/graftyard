// B0-24a on the Spectrum ingress (`build/BUILD-PLAN.md`, M6-WP2; the plan in tests/m6/results/m6-wp2.md): native git
// over SSH to ssh.graftyard.dev with the stock OpenSSH client and stock git, while one of the three `SshFront`s is
// killed every 10 minutes through the tools edge's `/ssh_kill` (the test-faults Registry `POST /ssh_front_kill`, which
// a site deployed with `--test-faults --ssh` answers). Resumable: state and logs live in <dir>, and a restart takes up
// where it stopped. Run it as a supervised orb service.
//
//   node tests/m6/hosted/b024a.mjs setup <dir>                  a new key on m3-member, a repository with a README
//   node tests/m6/hosted/b024a.mjs load <dir> [--hours 24] [--kill-every-min 10] [--row-from <ISO>]
//   node tests/m6/hosted/b024a.mjs report <dir>                 <dir>/summary.json, by b024a-judge.mjs
//   node tests/m6/hosted/b024a.mjs bf52 <dir>                   BF-52's revocation half, on its own key and repository
//   node tests/m6/hosted/b024a.mjs kill <dir> <site>            one kill (teardown), printed
//   node tests/m6/hosted/b024a.mjs drop-key <dir>               DELETE /user/keys/{id} for the row's key
//
// The smoke is `load` with `--hours 0.25 --kill-every-min 4`: about 300 sessions and one kill on each site.
// Environment as tests/am1/lf13/env-m3f.sh, with TOOLS_TOKEN_FILE, and ~/.env for the cost stops (acctcost.py, read
// from this driver, never from a request path). Tokens ride in headers and a credential helper only (`lib.mjs`).
//
// A session succeeds when git exits 0; a push also needs the remote's porcelain report to name its ref as updated,
// and is then acknowledged. Every acknowledged (ref, commit) must appear in a committed entry of the repository
// leader's log (test-faults `/log`, tailed every minute). A deleter removes verified branches over HTTPS, 50 every 5
// minutes, so the ref count, and with it the session time d, stays flat. Every 10 minutes the driver reads the cost:
// the row from `--row-from` over $10, graft-m3f's UTC day over $40, or the account's over $200 stops the load.
import { spawn, execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import { EDGE, Git, api, log, logFrom, leaderOf, row, sleep, state as m3, tokens, tools, voterAt } from "../../am1/hosted/lib.mjs";
import { judge } from "./b024a-judge.mjs";

const HOST = process.env.HOST ?? "ssh.graftyard.dev";
const SESSION_TIMEOUT_MS = 90_000;
const PERIOD_MS = 6_000; // each of the two workers starts one session this often
const WORKERS = 2;
const SITES = [0, 1, 2];
const STOPS = { row: 10, install: 40, account: 200 };
const ROOT = resolve(new URL("../../..", import.meta.url).pathname);

// Run as a script, the arguments are the command line; imported (b024a.test.mjs), there are none and nothing runs.
const isMain = import.meta.url === pathToFileURL(process.argv[1] ?? "").href;
const { values: opt, positionals } = parseArgs({ args: isMain ? process.argv.slice(2) : [], allowPositionals: true, options: {
  hours: { type: "string", default: "24" }, "kill-every-min": { type: "string", default: "10" }, "row-from": { type: "string" },
} });
const [cmd, dirArg, extra] = positionals;
const dir = resolve(dirArg ?? ".");
if (isMain) mkdirSync(dir, { recursive: true });
const P = (f) => join(dir, f);
const jsonl = (f) => (existsSync(P(f)) ? readFileSync(P(f), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : []);
const readJson = (f) => JSON.parse(readFileSync(P(f), "utf8"));
const save = (f, v) => writeFileSync(P(f), JSON.stringify(v));

// ---- git over SSH ----

function sshCommand(s) {
  return `ssh -i ${s.key} -o IdentitiesOnly=yes -o UserKnownHostsFile=${s.known} -o StrictHostKeyChecking=yes -o BatchMode=yes `
    + "-o ConnectTimeout=30 -o ServerAliveInterval=15";
}

// git in its own process group, killed whole at the session timeout, as B0-24a's driver learned (tests/m0/run/wp7.ts).
function git(args, cwd, s, env = {}) {
  return new Promise((done) => {
    const p = spawn("git", ["-c", "user.name=b024a", "-c", "user.email=b024a@x", ...args], { cwd, detached: true, stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, GIT_SSH_COMMAND: sshCommand(s), GIT_TERMINAL_PROMPT: "0", ...env } });
    let out = "", over = false;
    const finish = (code) => { if (over) return; over = true; clearTimeout(t); done({ code: code ?? -1, out }); };
    p.stdout.on("data", (d) => { out += d; });
    p.stderr.on("data", (d) => { out += d; });
    const t = setTimeout(() => {
      out += "\n[b024a: session timeout]";
      try { process.kill(-p.pid, "SIGKILL"); } catch { /* gone */ }
      setTimeout(() => finish(-1), 5000);
    }, SESSION_TIMEOUT_MS);
    p.on("close", finish);
  });
}

const remote = (s) => `git@${HOST}:${s.repo}.git`;

async function newKey(s, title) {
  execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-C", title, "-f", s.key]);
  const r = await api("POST", "/api/v3/user/keys", tokens.member, { title, key: readFileSync(`${s.key}.pub`, "utf8").trim() });
  if (r.status !== 201) throw new Error(`key add: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  return r.body.id;
}

// The host key pinned: the active slot's ED25519 key, as the edge lists it.
async function pinHost(s) {
  const m = await api("GET", "/api/graftyard/v1/meta/ssh");
  if (m.status !== 200) throw new Error(`meta/ssh: ${m.status}`);
  const k = (m.body.slots[m.body.active] ?? []).find((x) => x.type === "ssh-ed25519");
  if (!k) throw new Error(`no ssh-ed25519 key in slot ${m.body.active}`);
  writeFileSync(s.known, `${HOST} ${k.key}\n`);
  return { slot: m.body.active, fingerprint: k.fingerprint };
}

async function newRepo(name) {
  const r = await api("POST", "/api/v3/user/repos", tokens.member, { name, private: false, auto_init: true });
  if (r.status !== 201) throw new Error(`repo create: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
  return { repo: r.body.full_name, id: r.body.id, chain: `repo:${r.body.id}` };
}

async function setup() {
  const tag = new Date().toISOString().slice(5, 16).replace(/[-:T]/g, "");
  const s = { tag, key: P("id"), known: P("known_hosts") };
  s.keyId = await newKey(s, `b024a ${tag}`);
  Object.assign(s, await newRepo(`b024a-${tag}`), { host: await pinHost(s) });
  save("setup.json", s);
  log(`b024a setup: key ${s.keyId}, ${s.repo} (${s.chain}), host key slot ${s.host.slot} ${s.host.fingerprint}`);
}

// ---- one session ----

async function session(s, n, worker, op, work) {
  const t0 = Date.now();
  const x = { n, worker, op, start: t0, ms: 0, ok: false };
  let r;
  if (op === "clone") {
    const d = mkdtempSync(join(tmpdir(), "b024a-clone-"));
    r = await git(["clone", "-q", remote(s), join(d, "c")], d, s);
    rmSync(d, { recursive: true, force: true });
  } else if (op === "fetch") {
    r = await git(["fetch", "-q", "origin", "main"], work, s);
  } else {
    x.ref = `refs/heads/b024a/${n}`;
    writeFileSync(join(work, "push.txt"), `${s.tag} ${n} ${new Date(t0).toISOString()}\n`);
    await git(["add", "push.txt"], work, s);
    await git(["commit", "-q", "-m", `b024a ${n}`], work, s);
    x.sha = (await git(["rev-parse", "HEAD"], work, s)).out.trim();
    x.start = Date.now(); // the session is the push; the local commit is not one
    r = await git(["push", "--porcelain", "origin", `HEAD:${x.ref}`], work, s);
    await git(["reset", "-q", "--hard", "origin/main"], work, s);
    x.acked = r.code === 0 && r.out.split("\n").some((l) => /^[* +]\t/.test(l) && l.split("\t")[1]?.endsWith(`:${x.ref}`));
  }
  x.ms = Date.now() - x.start;
  x.ok = r.code === 0 && (op !== "push" || x.acked);
  if (!x.ok) x.err = r.out.replace(/\s+/g, " ").trim().slice(-600);
  return x;
}

// ---- the load ----

async function costs(from) {
  const read = (a, b) => JSON.parse(execFileSync("python3", [join(ROOT, "tests/fanout/acctcost.py"), a, b],
    { env: { ...process.env, PREFIX: "graft-m3f" }, encoding: "utf8" }).split("\n")[0]);
  const now = new Date().toISOString().slice(0, 19) + "Z";
  const day = read(now.slice(0, 10) + "T00:00:00Z", now);
  const rowCost = from ? read(from, now) : day;
  return { at: now, row: rowCost.prefix_usd, install: day.prefix_usd, account: day.usd };
}

async function killOnce(site) {
  const before = Date.now();
  const k = await tools("/ssh_kill", { site });
  const after = Date.now();
  if (k.status !== 200) throw new Error(`site ${site}: ssh_kill answered ${k.status} ${JSON.stringify(k.v).slice(0, 200)}`);
  return { site, ...k.v, orbClockOffsetMs: k.v.at - Math.round((before + after) / 2) };
}

async function load() {
  const s = readJson("setup.json");
  const everyMs = Number(opt["kill-every-min"]) * 60_000;
  const st = existsSync(P("state.json")) ? readJson("state.json") : {
    started: Date.now(), endAt: Date.now() + Number(opt.hours) * 3_600_000, n: 0, kills: 0, nextKill: Date.now() + everyMs,
    logFrom: 1, pauses: [], lastTick: Date.now(), costFails: 0, rowFrom: opt["row-from"] ?? null,
  };
  const keep = () => save("state.json", st);
  // A restart after the orb paused is a pause too; the next kill is one period away, not a burst of missed ones.
  if (Date.now() - st.lastTick > 30_000) st.pauses.push([st.lastTick, Date.now()]);
  if (st.nextKill < Date.now()) st.nextKill = Date.now() + everyMs;
  keep();
  log(`b024a load ${st.stopped ? "already stopped" : "start/resume"}: n=${st.n} kills=${st.kills} until ${new Date(st.endAt).toISOString()}`);
  const live = () => !st.stopped && Date.now() < st.endAt;

  const works = [];
  for (let w = 0; w < WORKERS; w++) {
    const wd = P(`work-${w}`);
    while (!existsSync(join(wd, ".git"))) {
      const r = await git(["clone", "-q", remote(s), wd], dir, s);
      if (r.code !== 0) { log(`work clone ${w}: ${r.out.slice(-200)}`); rmSync(wd, { recursive: true, force: true }); await sleep(5000); }
    }
    works.push(wd);
  }
  const ops = ["clone", "fetch", "push"];
  const worker = async (w) => {
    await sleep(w * (PERIOD_MS / WORKERS));
    while (live()) {
      const t0 = Date.now();
      const n = ++st.n; keep();
      row(P("sessions.jsonl"), await session(s, n, w, ops[n % 3], works[w]));
      await sleep(Math.max(0, PERIOD_MS - (Date.now() - t0)));
    }
  };
  const ticker = async () => {
    while (live()) {
      const now = Date.now();
      if (now - st.lastTick > 30_000) { st.pauses.push([st.lastTick, now]); log(`orb paused ${(now - st.lastTick) / 1000} s`); }
      st.lastTick = now; keep();
      await sleep(1000);
    }
  };
  const killer = async () => {
    while (live()) {
      if (Date.now() < st.nextKill) { await sleep(Math.min(5000, st.nextKill - Date.now())); continue; }
      const site = SITES[st.kills % SITES.length];
      st.kills++; st.nextKill += everyMs; keep();
      let k;
      try { k = await killOnce(site); } catch (e) { k = { site, error: String(e).slice(0, 300), at: Date.now(), orbClockOffsetMs: 0 }; }
      const t = k.at - k.orbClockOffsetMs; // the kill on the orb's clock
      // Back when the front answers its banner again; the next session routed to it starts it (`ssh.mjs`, `open`).
      for (let i = 0; !k.error && k.running && i < 1000 && Date.now() < st.nextKill - 5000; i++) {
        await sleep(500);
        try {
          const r = await tools("/ssh_status", { site });
          if (r.v?.running && r.v?.banner) { k.restartMs = Date.now() - t; k.banner = r.v.banner; break; }
        } catch { /* keep polling */ }
      }
      row(P("kills.jsonl"), { kill: st.kills, ...k });
      log(`kill ${st.kills} site ${site}: open ${k.open ?? "?"} restart ${k.restartMs ?? "-"} ms${k.error ? ` ${k.error}` : ""}`);
    }
  };
  // Acknowledged pushes against the leader's committed log.
  const logged = new Set(jsonl("logged.jsonl").map((x) => `${x.ref} ${x.sha}`));
  const checker = async () => {
    while (live() || st.checkAfterEnd !== true) {
      try {
        for (;;) {
          const page = await logFrom(s.chain, st.logFrom, 500);
          let moved = 0;
          for (const e of page) {
            if (!e.committed) break;
            for (const [ref, sha] of e.refs ?? []) if (sha) { logged.add(`${ref} ${sha}`); row(P("logged.jsonl"), { index: e.index, ref, sha }); }
            // The test `/log` lists an entry's first 8 ref updates only (`workers/site/src/family.rs`, `log_fields`).
            // A deleter's batch lists deletes only, which no acknowledged push needs.
            if ((e.ref_updates ?? 0) > (e.refs ?? []).length && (e.refs ?? []).some(([, sha]) => sha)) {
              row(P("truncated.jsonl"), { index: e.index, refUpdates: e.ref_updates });
            }
            st.logFrom = e.index + 1; moved++;
          }
          keep();
          if (page.length < 500 || moved < page.length) break;
        }
      } catch (e) { log(`log tail: ${String(e).slice(0, 200)}`); }
      if (!live()) { st.checkAfterEnd = true; keep(); break; }
      await sleep(60_000);
    }
  };
  // Verified branches go over HTTPS, 50 at a time; these pushes are not sessions.
  const deleter = async () => {
    const g = new Git(s.repo).as(tokens.member);
    // `runAsync`, not `run`: a synchronous git stops the event loop and lengthens every session in flight.
    for (let i = 0; i < 5 && !(await g.runAsync(["clone", "-q", g.url, g.wt], g.dir)).ok; i++) await sleep(3000);
    const gone = new Set(jsonl("deleted.jsonl").map((x) => x.ref));
    while (live()) {
      await sleep(5 * 60_000);
      const batch = jsonl("sessions.jsonl").filter((x) => x.acked && !gone.has(x.ref) && logged.has(`${x.ref} ${x.sha}`)).slice(0, 50);
      if (!batch.length) continue;
      const r = await g.runAsync(["push", "-q", "origin", ...batch.map((x) => `:${x.ref}`)]);
      if (r.ok) for (const x of batch) { gone.add(x.ref); row(P("deleted.jsonl"), { ref: x.ref, at: Date.now() }); }
      else log(`delete batch: ${r.out.slice(0, 200)}`);
    }
  };
  const stops = async () => {
    while (live()) {
      try {
        const c = await costs(st.rowFrom);
        row(P("cost.jsonl"), c);
        st.costFails = 0;
        const over = Object.entries(STOPS).find(([k, cap]) => c[k] >= cap);
        if (over) { st.stopped = { at: Date.now(), why: `${over[0]} $${c[over[0]]} reached its $${over[1]} stop` }; log(`STOP: ${st.stopped.why}`); }
      } catch (e) {
        // A stop the driver cannot read is a stop.
        if (++st.costFails >= 3) { st.stopped = { at: Date.now(), why: `the cost read failed 3 times: ${String(e).slice(0, 200)}` }; log(`STOP: ${st.stopped.why}`); }
      }
      keep();
      for (let i = 0; i < 600 && live(); i++) await sleep(1000);
    }
  };
  await Promise.all([...works.map((_, w) => worker(w)), ticker(), killer(), checker(), deleter(), stops()]);
  log(`b024a load end: n=${st.n} kills=${st.kills}${st.stopped ? `, stopped: ${st.stopped.why}` : ""}`);
}

function report() {
  const st = readJson("state.json");
  const out = judge({
    sessions: jsonl("sessions.jsonl"),
    kills: jsonl("kills.jsonl").filter((k) => !k.error),
    pauses: st.pauses,
    logged: jsonl("logged.jsonl").map((x) => `${x.ref} ${x.sha}`),
  });
  out.stopped = st.stopped ?? null;
  out.killErrors = jsonl("kills.jsonl").filter((k) => k.error).length;
  // An acknowledged push missing from the log while some entry listed only its first 8 refs is read by hand first.
  out.truncatedEntries = jsonl("truncated.jsonl").length;
  out.lastCost = jsonl("cost.jsonl").at(-1) ?? null;
  save("summary.json", out);
  console.log(JSON.stringify(out, null, 1));
}

// ---- BF-52's revocation half ----

async function bf52() {
  const N = 1000;
  const s = { tag: `bf52-${Date.now().toString(36)}`, key: P("bf52-id"), known: P("known_hosts") };
  s.keyId = await newKey(s, `b024a ${s.tag}`);
  Object.assign(s, await newRepo(`b024a-${s.tag}`));
  if (!existsSync(s.known)) await pinHost(s);
  const out = (v) => { row(P("bf52.jsonl"), v); log(`bf52: ${JSON.stringify(v).slice(0, 300)}`); };
  out({ step: "setup", key: s.keyId, repo: s.repo, chain: s.chain });
  // Control: a real SSH push with the key is accepted.
  const wt = mkdtempSync(join(tmpdir(), "b024a-bf52-"));
  const c = await git(["clone", "-q", remote(s), join(wt, "c")], wt, s);
  writeFileSync(join(wt, "c", "bf52.txt"), `${s.tag}\n`);
  await git(["add", "bf52.txt"], join(wt, "c"), s);
  await git(["commit", "-q", "-m", "bf52 control"], join(wt, "c"), s);
  const ctl = await git(["push", "-q", "origin", "HEAD:refs/heads/control"], join(wt, "c"), s);
  out({ step: "control push over ssh", clone: c.code, push: ctl.code, err: ctl.code ? ctl.out.slice(-200) : undefined });
  if (c.code || ctl.code) throw new Error("the control push failed; BF-52 needs a key that pushes first");
  // The queued pushes, built with graftyard_git's encoder under the key's principal.
  const bin = (args) => JSON.parse(execFileSync("cargo", ["run", "-q", "--release", "--bin", "bf52queue", "--", ...args],
    { cwd: join(ROOT, "tests/m3/hosted/tools"), encoding: "utf8", maxBuffer: 64 << 20 }));
  const site = await leaderOf(s.chain);
  const leader = { site, do_name: voterAt(s.chain, site) ?? `${s.chain}#1#f${site}` };
  // A queue control, no hold and no revocation: 10 such pushes are acknowledged, so the 401s below are the key's.
  const q0 = bin([String(m3.member), `${s.key}.pub`, "10", `${s.tag}-ok`]);
  const r0 = await tools("/commit_many", { chain: s.chain, leader, cmds: q0.cmds.map((x) => x.cmd), rids: q0.cmds.map((x) => x.rid) });
  const ok0 = (r0.results ?? []).filter((x) => x.done?.ok).length;
  out({ step: "queue control", fingerprint: q0.fingerprint, leader, acked: ok0, of: q0.cmds.length });
  if (ok0 !== q0.cmds.length) throw new Error("the queue control was not all acknowledged");
  const q = bin([String(m3.member), `${s.key}.pub`, String(N), s.tag]);
  // `tools` re-sends a call that failed below HTTP; the request ids are fixed, so a re-send gets the same outcomes.
  const queued = tools("/commit_many", { chain: s.chain, leader, cmds: q.cmds.map((x) => x.cmd), rids: q.cmds.map((x) => x.rid), hold_deps_ms: 8000, hold_ms: 60_000 });
  await sleep(2000);
  const del = await api("DELETE", `/api/v3/user/keys/${s.keyId}`, tokens.member);
  out({ step: "key delete", status: del.status, at: new Date().toISOString() });
  const r = await queued;
  const res = r.results ?? [];
  const by = { ok: 0, refused401: 0, other: 0 };
  for (const x of res) x.done?.ok ? by.ok++ : x.done?.refused?.status === 401 ? by.refused401++ : by.other++;
  // No ref of the queue moved: the repository lists none over HTTPS (the test `/log` names only 8 refs an entry).
  const g = new Git(s.repo).as(tokens.member);
  const ls = g.run(["ls-remote", "--heads", g.url], g.dir);
  if (!ls.ok) throw new Error(`ls-remote: ${ls.out.slice(0, 200)}`);
  const moved = ls.out.split("\n").filter((l) => l.split("\t")[1]?.startsWith(`refs/heads/${s.tag}/`));
  out({ step: "queue", results: res.length, ...by, refsMoved: moved.length });
  // The key is gone for real SSH too.
  const after = await git(["clone", "-q", remote(s), join(wt, "c2")], wt, s);
  writeFileSync(join(wt, "c", "late.txt"), "late\n");
  await git(["add", "late.txt"], join(wt, "c"), s);
  await git(["commit", "-q", "-m", "after delete"], join(wt, "c"), s);
  const late = await git(["push", "-q", "origin", "HEAD:refs/heads/late"], join(wt, "c"), s);
  const denied = (x) => x.code !== 0 && /Permission denied/.test(x.out);
  out({ step: "after delete", cloneDenied: denied(after), pushDenied: denied(late) });
  rmSync(wt, { recursive: true, force: true });
  const pass = res.length === N && by.refused401 === N && moved.length === 0 && del.status === 204 && denied(after) && denied(late);
  out({ step: "verdict", pass, repo: s.repo, repoId: s.id });
}

// Every subcommand, by name: the usage line and b024a.test.mjs read this table.
export const COMMANDS = {
  setup,
  load,
  report: async () => report(),
  bf52,
  kill: async () => console.log(JSON.stringify(await killOnce(Number(extra)))),
  "drop-key": async () => {
    const s = readJson("setup.json");
    console.log((await api("DELETE", `/api/v3/user/keys/${s.keyId}`, tokens.member)).status);
  },
};

if (isMain) {
  if (!Object.hasOwn(COMMANDS, cmd)) {
    console.error(`usage: b024a.mjs ${Object.keys(COMMANDS).join("|")} <dir> [kill: <site>]`);
    process.exit(2);
  }
  await COMMANDS[cmd]();
}
