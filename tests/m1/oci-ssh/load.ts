// git over plain SSH through the Oracle proxy (deploy/oci-ssh), against the M0-WP7 bed `graft-m0-ssh`, with the M0
// client path (`cloudflared access ssh` as ProxyCommand) run in the same window as the control.
//
//   node tests/m1/oci-ssh/load.ts load --run X [--minutes 25] [--restart-at 12.5]
//   node tests/m1/oci-ssh/load.ts report --run X
//
// Two workers, one per path, each one session every 6 s, rotating clone, fetch, and push: the same operations, rate,
// and success rule as tests/m0/run/wp7.ts (git exits 0, the serving front names itself, a push reports the group seq).
// At --restart-at minutes the runner opens an SSH session through the proxy and holds it, starts a clone, restarts
// graft-ssh-proxy.service over admin SSH, and probes `git ls-remote` every 0.5 s for 20 s, to see what open sessions
// and new connections get.
// Raw data goes to tests/m1/results/raw/oci-ssh-<date>/<run>/ (git-ignored).

import { spawn } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { parseArgs } from "node:util";

const REPO = new URL("../../../", import.meta.url).pathname;
const GEN = `${REPO}tests/m0/.gen/wp7`;
const PROXY_HOST = "ssh.graftyard.dev";
const TUNNEL_HOST = "ssh-m0.graftyard.dev";
const ADMIN = { host: "<ssh-proxy-ip>", port: "2222", key: `${homedir()}/.ssh/graftyard-ssh-proxy`, known: `${homedir()}/.ssh/known_hosts.graft` };
const SESSION_TIMEOUT_MS = 90_000;
const CYCLE_MS = 6000;

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, minutes: { type: "string", default: "25" }, "restart-at": { type: "string", default: "12.5" },
  date: { type: "string", default: "20261103" },
} });
const DIR = `${REPO}tests/m1/results/raw/oci-ssh-${opt.date}/${opt.run}`;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const now = () => new Date().toISOString();

type Path = "proxy" | "cloudflared";
type Op = "clone" | "fetch" | "push";
interface Session { n: number; path: Path; op: Op; start: string; ms: number; ok: boolean; front?: string; seq?: number; err?: string; tag?: string }

// Both configs keep the M0 stanza's timeouts. The proxy one has no ProxyCommand: plain ssh to port 22.
function sshConfig(path: Path): string {
  const p = `${DIR}/ssh_config.${path}`;
  const host = path === "proxy" ? PROXY_HOST : TUNNEL_HOST;
  writeFileSync(`${DIR}/known_hosts.${path}`, readFileSync(`${GEN}/known_hosts`, "utf8").replace(/^\S+/gm, host));
  writeFileSync(p, [
    path === "proxy"
      ? `Host ${PROXY_HOST}\n  User git\n  ServerAliveInterval 15\n  ServerAliveCountMax 4\n  ConnectTimeout 30`
      : `Include ${REPO}tests/m0/wp7/ssh_config`,
    "", "Host *", `  IdentityFile ${GEN}/client_ed25519`, "  IdentitiesOnly yes", `  UserKnownHostsFile ${DIR}/known_hosts.${path}`,
    "  StrictHostKeyChecking yes", "  LogLevel VERBOSE", ""].join("\n"));
  return p;
}

const envs: Partial<Record<Path, NodeJS.ProcessEnv>> = {};
function gitEnv(path: Path): NodeJS.ProcessEnv {
  return envs[path] ??= { ...process.env, PATH: `${GEN}:${process.env.PATH}`, GIT_SSH_COMMAND: `ssh -F ${sshConfig(path)}`,
    GIT_TERMINAL_PROMPT: "0", GIT_AUTHOR_NAME: "oci-ssh", GIT_AUTHOR_EMAIL: "oci-ssh@m1", GIT_COMMITTER_NAME: "oci-ssh", GIT_COMMITTER_EMAIL: "oci-ssh@m1" };
}

function git(path: Path, args: string[], cwd: string): Promise<{ code: number; out: string }> {
  return new Promise((resolve) => {
    // Own process group, so a timeout takes ssh and cloudflared down with git (tests/m0/results/m0-wp7.md, B0-24a).
    const p = spawn("git", args, { cwd, env: gitEnv(path), stdio: ["ignore", "pipe", "pipe"], detached: true });
    let out = "", done = false;
    const finish = (code: number | null) => { if (done) return; done = true; clearTimeout(t); resolve({ code: code ?? -1, out }); };
    p.stdout.on("data", (d) => { out += d; }); p.stderr.on("data", (d) => { out += d; });
    const t = setTimeout(() => {
      out += "\n[oci-ssh: session timeout]";
      try { process.kill(-p.pid!, "SIGKILL"); } catch { /* already gone */ }
      setTimeout(() => finish(-1), 5000);
    }, SESSION_TIMEOUT_MS);
    p.on("close", (code) => finish(code));
  });
}

const remote = (path: Path) => `${path === "proxy" ? PROXY_HOST : TUNNEL_HOST}:acme/app.git`;

async function session(run: string, n: number, path: Path, op: Op, work: string): Promise<Session> {
  const t0 = Date.now();
  const s: Session = { n, path, op, start: new Date(t0).toISOString(), ms: 0, ok: false };
  let r: { code: number; out: string };
  if (op === "clone") {
    const dir = mkdtempSync(`${tmpdir()}/oci-ssh-clone-`);
    r = await git(path, ["clone", "-q", remote(path), `${dir}/c`], dir);
    rmSync(dir, { recursive: true, force: true });
  } else if (op === "fetch") {
    r = await git(path, ["fetch", "-q", "origin", "main"], work);
  } else {
    writeFileSync(`${work}/push.txt`, `${run} ${n} ${now()}\n`);
    await git(path, ["add", "push.txt"], work);
    await git(path, ["commit", "-q", "-m", `oci-ssh ${run} ${n}`], work);
    r = await git(path, ["push", "-q", "origin", `HEAD:refs/heads/oci-ssh/${run}/${t0}-${n}`], work);
    await git(path, ["reset", "-q", "--hard", "origin/main"], work);
    const m = /committed seq=(\d+)/.exec(r.out);
    if (m) s.seq = Number(m[1]);
  }
  s.ms = Date.now() - t0;
  s.front = /graft-front=(front-[abc])/.exec(r.out)?.[1];
  s.ok = r.code === 0 && !!s.front && (op !== "push" || s.seq !== undefined);
  if (!s.ok) s.err = r.out.replace(/\s+/g, " ").trim().slice(-1500);
  return s;
}

function admin(cmd: string): Promise<string> {
  return new Promise((resolve) => {
    const p = spawn("ssh", ["-p", ADMIN.port, "-i", ADMIN.key, "-o", "IdentitiesOnly=yes", "-o", `UserKnownHostsFile=${ADMIN.known}`,
      "-o", "ConnectTimeout=10", `ubuntu@${ADMIN.host}`, cmd], { stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    p.stdout.on("data", (d) => { out += d; }); p.stderr.on("data", (d) => { out += d; });
    p.on("close", () => resolve(out));
  });
}

// An SSH session open across the restart: upload-pack has sent its ref advertisement and waits for the client's wants.
function heldSession(): Promise<Session> {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const p = spawn("ssh", ["-F", sshConfig("proxy"), PROXY_HOST, "git-upload-pack 'acme/app.git'"], { stdio: ["pipe", "pipe", "pipe"] });
    let out = "", advertised = 0;
    p.stdout.on("data", (d) => { if (!advertised && /refs\/heads\/main/.test(String(d))) advertised = Date.now() - t0; });
    p.stderr.on("data", (d) => { out += d; });
    const t = setTimeout(() => p.kill("SIGKILL"), 40_000);
    p.on("close", (code) => {
      clearTimeout(t);
      resolve({ n: -1, path: "proxy", op: "fetch", tag: `held upload-pack (refs after ${advertised} ms)`, start: new Date(t0).toISOString(),
        ms: Date.now() - t0, ok: code === null, err: `exit ${code}: ${out.replace(/\s+/g, " ").trim().slice(-400)}` });
    });
  });
}

async function restartProbe(run: string, work: string): Promise<void> {
  const log = (o: object) => appendFileSync(`${DIR}/restart.jsonl`, JSON.stringify({ at: now(), ...o }) + "\n");
  const held = heldSession();
  await sleep(4000);
  const inflight = session(run, -1, "proxy", "clone", work).then((s) => ({ ...s, tag: "clone started 1 s before restart" }));
  await sleep(1000);
  const probes: Promise<Session>[] = [];
  const probeEnd = Date.now() + 20_000;
  const prober = (async () => {
    for (let i = 0; Date.now() < probeEnd; i++) {
      const t0 = Date.now();
      probes.push(git("proxy", ["ls-remote", remote("proxy"), "main"], work).then((r) => ({
        n: -100 - i, path: "proxy" as Path, op: "fetch" as Op, tag: "ls-remote probe", start: new Date(t0).toISOString(), ms: Date.now() - t0,
        ok: r.code === 0 && /refs\/heads\/main/.test(r.out), front: /graft-front=(front-[abc])/.exec(r.out)?.[1],
        err: r.code === 0 ? undefined : r.out.replace(/\s+/g, " ").trim().slice(-600) })));
      await sleep(500);
    }
  })();
  const before = Date.now();
  log({ event: "restart issued" });
  // VM-clock times: before the restart, and when cloudflared is listening on 22 again.
  const out = await admin("date +%s.%N; sudo systemctl restart graft-ssh-proxy.service; " +
    "for i in $(seq 200); do sudo ss -Htlnp 'sport = :22' | grep -q cloudflared && break; sleep 0.02; done; date +%s.%N; " +
    "systemctl show graft-ssh-proxy -p MainPID -p NRestarts");
  const [t1, t2] = out.trim().split("\n").map(Number);
  log({ event: "restart done", adminRoundTripMs: Date.now() - before, vmRestartToListenMs: Math.round((t2 - t1) * 1000), vm: out.trim().split("\n") });
  await prober;
  for (const s of [await held, await inflight, ...(await Promise.all(probes))]) appendFileSync(`${DIR}/restart-sessions.jsonl`, JSON.stringify(s) + "\n");
}

async function load(): Promise<void> {
  const run = opt.run!;
  mkdirSync(DIR, { recursive: true });
  appendFileSync(`${DIR}/log.txt`, `${now()} load start\n`);
  const endAt = Date.now() + Number(opt.minutes) * 60_000;
  const restartAt = Date.now() + Number(opt["restart-at"]) * 60_000;
  const paths: Path[] = ["proxy", "cloudflared"];
  const works: Record<Path, string> = { proxy: `${DIR}/work-proxy`, cloudflared: `${DIR}/work-cloudflared` };
  for (const p of paths) while (!existsSync(`${works[p]}/.git`)) {
    const r = await git(p, ["clone", "-q", remote(p), works[p]], DIR);
    if (r.code !== 0) { appendFileSync(`${DIR}/log.txt`, `${now()} ${p} work clone failed: ${r.out.slice(-300)}\n`); rmSync(works[p], { recursive: true, force: true }); await sleep(5000); }
  }
  const probeWork = `${DIR}/work-probe`;
  if (!existsSync(`${probeWork}/.git`)) await git("proxy", ["clone", "-q", remote("proxy"), probeWork], DIR);
  let n = 0;
  const ops: Op[] = ["clone", "fetch", "push"];
  const loop = async (path: Path, offset: number) => {
    await sleep(offset);
    for (let i = 0; Date.now() < endAt; i++) {
      const t0 = Date.now();
      const s = await session(run, ++n, path, ops[i % 3], works[path]);
      appendFileSync(`${DIR}/sessions.jsonl`, JSON.stringify(s) + "\n");
      await sleep(Math.max(0, CYCLE_MS - (Date.now() - t0)));
    }
  };
  const restarter = async () => { await sleep(restartAt - Date.now()); await restartProbe(run, probeWork); };
  await Promise.all([loop("proxy", 0), loop("cloudflared", CYCLE_MS / 2), restarter()]);
  appendFileSync(`${DIR}/log.txt`, `${now()} load end n=${n}\n`);
}

function jsonl<T>(p: string): T[] {
  return existsSync(p) ? readFileSync(p, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
}

function report(): void {
  const ss = jsonl<Session>(`${DIR}/sessions.jsonl`);
  const pct = (a: number[], p: number) => { const x = [...a].sort((u, v) => u - v); return x[Math.min(x.length - 1, Math.floor(p * x.length))]; };
  const out: Record<string, unknown> = { run: opt.run, first: ss[0]?.start, last: ss.at(-1)?.start };
  for (const path of ["proxy", "cloudflared"] as Path[]) {
    const p = ss.filter((s) => s.path === path);
    const byOp: Record<string, object> = {};
    for (const op of ["clone", "fetch", "push"] as Op[]) {
      const o = p.filter((s) => s.op === op), g = o.filter((s) => s.ok).map((s) => s.ms);
      byOp[op] = { sessions: o.length, ok: g.length, p50: pct(g, 0.5), p99: pct(g, 0.99), max: Math.max(...g) };
    }
    const fronts: Record<string, number> = {};
    for (const s of p.filter((x) => x.ok)) fronts[s.front!] = (fronts[s.front!] ?? 0) + 1;
    out[path] = { sessions: p.length, ok: p.filter((s) => s.ok).length, successRate: p.filter((s) => s.ok).length / p.length, byOp, fronts,
      failures: p.filter((s) => !s.ok).map((s) => ({ n: s.n, op: s.op, start: s.start, ms: s.ms, err: s.err?.slice(-300) })) };
  }
  out.restart = { events: jsonl(`${DIR}/restart.jsonl`), sessions: jsonl<Session>(`${DIR}/restart-sessions.jsonl`)
    .map((s) => ({ tag: s.tag, start: s.start, ms: s.ms, ok: s.ok, err: s.err?.slice(-200) })) };
  writeFileSync(`${DIR}/summary.json`, JSON.stringify(out, null, 1));
  console.log(JSON.stringify(out, null, 1));
}

if (positionals[0] === "load") await load();
else if (positionals[0] === "report") report();
else { console.error("usage: node tests/m1/oci-ssh/load.ts load|report --run X"); process.exit(2); }
