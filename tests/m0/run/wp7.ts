// M0-WP7 (B0-24) runner: the SSH front bed `graft-m0-ssh` (src/wp7/ssh.ts, wp7/) and its load.
//
//   node run/wp7.ts spectrum        try the design's Spectrum app on graftyard.dev; writes results/raw/wp7/spectrum-*.json
//   node run/wp7.ts deploy          deploy graft-m0-ssh, set its secrets, register the client key, start three fronts
//   node run/wp7.ts status          each front's container, cloudflared readiness, and the tunnel's connectors
//   node run/wp7.ts load --run X [--hours 2] [--gap 3]   resumable load: SSH git sessions, one front killed every 10 min
//   node run/wp7.ts report --run X  summarize sessions, kills, and the group log into results/raw/wp7/<run>/summary.json
//   node run/wp7.ts stop            stop the three fronts (containers and alarms); keep the script, tunnel, and DNS
//   node run/wp7.ts delete          stop the fronts and delete the script, tunnel, route, virtual network, and DNS record
//
// The client path is the Tunnel only: `ssh` with `ProxyCommand cloudflared access ssh` to ssh-m0.graftyard.dev, a
// proxied CNAME to the tunnel, whose ingress sends SSH to sshd on the replica's loopback. Spectrum is not in the path,
// because the Pro zone refuses the design's Spectrum app (results/m0-wp7.md).
//
// Local state is in .gen/wp7/ (gitignored): the host key, the client key, known_hosts, and the cloudflared binary.

import { execFileSync, spawn } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { parseArgs } from "node:util";

const ROOT = new URL("../", import.meta.url).pathname;
const GEN = `${ROOT}.gen/wp7`;
const RAW = `${ROOT}results/raw/wp7`;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = `https://graft-m0-ssh.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const CF = "https://api.cloudflare.com/client/v4";
const ZONE = `${process.env.CF_ZONE_ID}`;
const ACCOUNT = `${process.env.CLOUDFLARE_ACCOUNT_ID}`;
const HOST = "ssh-m0.graftyard.dev";
const NAMES = { tunnel: "graft-m0-ssh", vnet: "graft-m0-ssh-vnet", ip: "10.222.0.22" };
const FRONTS = ["front-a", "front-b", "front-c"] as const;
const KILL_EVERY_MS = 10 * 60_000;
const SESSION_TIMEOUT_MS = 90_000;

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, hours: { type: "string", default: "2" }, gap: { type: "string", default: "3" },
} });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const now = () => new Date().toISOString();

async function cf(method: string, path: string, body?: unknown): Promise<any> {
  const r = await fetch(CF + path, { method, headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`,
    "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  return r.json();
}

async function harness(method: string, path: string, body?: unknown): Promise<any> {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(URL_ + path, { method, headers: { authorization: `Bearer ${TOKEN}` },
      body: body === undefined ? undefined : typeof body === "string" ? body : JSON.stringify(body), signal: AbortSignal.timeout(60_000) }).catch((e) => e as Error);
    if (!(r instanceof Error) && r.ok) return r.json();
    const why = r instanceof Error ? String(r) : `${r.status} ${(await r.text()).slice(0, 200)}`;
    if (attempt >= 3) throw new Error(`${path}: ${why}`);
    await sleep(3000);
  }
}

async function tunnelId(): Promise<string> {
  const t = await cf("GET", `/accounts/${ACCOUNT}/cfd_tunnel?is_deleted=false&name=${NAMES.tunnel}`);
  return t.result[0].id;
}

// The design's shape (DESIGN.md, Shape, "SSH front"): an SSH app on port 22 whose origin is the replicas' private IP in
// the tunnel's virtual network. Then the pay-as-you-go minimum (protocol, dns, origin_direct) the error docs allow.
async function spectrum(): Promise<void> {
  mkdirSync(RAW, { recursive: true });
  const vnets = await cf("GET", `/accounts/${ACCOUNT}/teamnet/virtual_networks?name=${NAMES.vnet}`);
  const vnet = vnets.result[0].id;
  const tries: Record<string, object> = {
    "vnet": { protocol: "tcp/22", dns: { type: "CNAME", name: "ssh.graftyard.dev" }, origin_direct: [`tcp://${NAMES.ip}:22`],
              virtual_network_id: vnet, proxy_protocol: "off", traffic_type: "direct" },
    "vnet-minimal": { protocol: "tcp/22", dns: { type: "CNAME", name: "ssh.graftyard.dev" }, origin_direct: [`tcp://${NAMES.ip}:22`],
                      virtual_network_id: vnet },
    "payg-minimal": { protocol: "tcp/22", dns: { type: "CNAME", name: "ssh.graftyard.dev" }, origin_direct: [`tcp://${NAMES.ip}:22`] },
  };
  for (const [k, body] of Object.entries(tries)) {
    const r = await cf("POST", `/zones/${ZONE}/spectrum/apps`, body);
    writeFileSync(`${RAW}/spectrum-${k}-${now().slice(0, 16).replace(/:/g, "")}.json`, JSON.stringify({ at: now(), body, response: r }, null, 1));
    console.log(k, JSON.stringify(r.errors ?? r.result));
    if (r.success) { console.log("created; the app stays for the load run"); break; }
  }
}

function hostKeyLine(): string {
  return readFileSync(`${GEN}/host_ed25519.pub`, "utf8").trim().split(" ").slice(0, 2).join(" ");
}

async function deploy(): Promise<void> {
  const tid = await tunnelId();
  const tok = await cf("GET", `/accounts/${ACCOUNT}/cfd_tunnel/${tid}/token`);
  execFileSync("npx", ["wrangler", "deploy", "-c", "wp7/graft-m0-ssh.jsonc"], { cwd: ROOT, stdio: "inherit" });
  const secrets = { M0_TOKEN: TOKEN, SSH_HOST_KEY: readFileSync(`${GEN}/host_ed25519`, "utf8"), TUNNEL_TOKEN: tok.result };
  execFileSync("npx", ["wrangler", "secret", "bulk", "-c", "wp7/graft-m0-ssh.jsonc"], { cwd: ROOT, input: JSON.stringify(secrets), stdio: ["pipe", "inherit", "inherit"] });
  const fp = execFileSync("ssh-keygen", ["-lf", `${GEN}/client_ed25519.pub`], { encoding: "utf8" }).split(" ")[1];
  await harness("POST", "/keys", { fp, line: readFileSync(`${GEN}/client_ed25519.pub`, "utf8").trim() });
  writeFileSync(`${GEN}/known_hosts`, `${HOST} ${hostKeyLine()}\n`);
  for (const f of FRONTS) console.log(f, JSON.stringify(await harness("POST", `/front/${f}/start`)));
}

async function status(): Promise<object> {
  const out: Record<string, unknown> = {};
  for (const f of FRONTS) out[f] = await harness("GET", `/front/${f}/status`);
  const c = await cf("GET", `/accounts/${ACCOUNT}/cfd_tunnel/${await tunnelId()}/connections`);
  out.connectors = (c.result ?? []).map((x: any) => ({ id: x.id, conns: (x.conns ?? []).map((k: any) => k.colo_name).join(","), origin_ip: x.conns?.[0]?.origin_ip }));
  return out;
}

// ---- load ----

interface Session { n: number; worker: number; op: "clone" | "fetch" | "push"; start: string; ms: number; ok: boolean;
  front?: string; seq?: number; sha?: string; err?: string }

// The client config is the docs' stanza (wp7/ssh_config) unchanged. The test adds only its own key, known_hosts, and
// verbose logging, in a block after the stanza, and puts the cloudflared binary on PATH, as a user's install would.
function sshConfig(): string {
  const p = `${GEN}/ssh_config`;
  writeFileSync(p, [`Include ${ROOT}wp7/ssh_config`, "", "Host *", `  IdentityFile ${GEN}/client_ed25519`, "  IdentitiesOnly yes",
    `  UserKnownHostsFile ${GEN}/known_hosts`, "  StrictHostKeyChecking yes", "  LogLevel VERBOSE", ""].join("\n"));
  return p;
}

function gitEnv(): NodeJS.ProcessEnv {
  return { ...process.env, PATH: `${GEN}:${process.env.PATH}`, GIT_SSH_COMMAND: `ssh -F ${sshConfig()}`, GIT_TERMINAL_PROMPT: "0",
           GIT_AUTHOR_NAME: "wp7", GIT_AUTHOR_EMAIL: "wp7@m0", GIT_COMMITTER_NAME: "wp7", GIT_COMMITTER_EMAIL: "wp7@m0" };
}

function git(args: string[], cwd: string): Promise<{ code: number; out: string }> {
  return new Promise((resolve) => {
    // Its own process group, so a timeout kills git, ssh, and cloudflared together. Killing only git once left ssh and
    // `cloudflared access` holding the output pipes, and the session never ended (results/m0-wp7.md, B0-24a).
    const p = spawn("git", args, { cwd, env: gitEnv(), stdio: ["ignore", "pipe", "pipe"], detached: true });
    let out = "", done = false;
    const finish = (code: number | null) => { if (done) return; done = true; clearTimeout(t); resolve({ code: code ?? -1, out }); };
    p.stdout.on("data", (d) => { out += d; }); p.stderr.on("data", (d) => { out += d; });
    const t = setTimeout(() => {
      out += "\n[wp7: session timeout]";
      try { process.kill(-p.pid!, "SIGKILL"); } catch { /* already gone */ }
      setTimeout(() => finish(-1), 5000);
    }, SESSION_TIMEOUT_MS);
    p.on("close", (code) => finish(code));
  });
}

const REMOTE = `${HOST}:acme/app.git`; // User git comes from the stanza

async function session(run: string, n: number, worker: number, op: Session["op"], work: string): Promise<Session> {
  const t0 = Date.now();
  const s: Session = { n, worker, op, start: new Date(t0).toISOString(), ms: 0, ok: false };
  let r: { code: number; out: string };
  if (op === "clone") {
    const dir = mkdtempSync(`${tmpdir()}/wp7-clone-`);
    r = await git(["clone", "-q", REMOTE, `${dir}/c`], dir);
    rmSync(dir, { recursive: true, force: true });
  } else if (op === "fetch") {
    r = await git(["fetch", "-q", "origin", "main"], work);
  } else {
    writeFileSync(`${work}/push.txt`, `${run} ${n} ${now()}\n`);
    await git(["add", "push.txt"], work);
    await git(["commit", "-q", "-m", `wp7 ${run} ${n}`], work);
    s.sha = (await git(["rev-parse", "HEAD"], work)).out.trim();
    r = await git(["push", "-q", "origin", `HEAD:refs/heads/wp7/${run}/${n}`], work);
    await git(["reset", "-q", "--hard", "origin/main"], work);
    const m = /committed seq=(\d+)/.exec(r.out);
    if (m) s.seq = Number(m[1]);
  }
  s.ms = Date.now() - t0;
  s.front = /graft-front=(front-[abc])/.exec(r.out)?.[1];
  s.ok = r.code === 0 && !!s.front && (op !== "push" || s.seq !== undefined);
  if (!s.ok) s.err = r.out.replace(/\s+/g, " ").trim().slice(-2500);
  return s;
}

async function load(): Promise<void> {
  const run = opt.run!;
  const dir = `${RAW}/${run}`;
  mkdirSync(dir, { recursive: true });
  const stateFile = `${dir}/state.json`;
  const state = existsSync(stateFile) ? JSON.parse(readFileSync(stateFile, "utf8"))
    : { started: Date.now(), endAt: Date.now() + Number(opt.hours) * 3600_000, nextKill: Date.now() + KILL_EVERY_MS, n: 0, kills: 0 };
  const save = () => writeFileSync(stateFile, JSON.stringify(state));
  // After an orb pause, take the next kill one period from now instead of a burst of missed kills.
  if (state.nextKill < Date.now()) state.nextKill = Date.now() + KILL_EVERY_MS;
  save();
  appendFileSync(`${dir}/log.txt`, `${now()} load start/resume n=${state.n}\n`);
  const gapMs = Number(opt.gap) * 1000;
  const last: Record<string, number> = {};
  const workers = 2;
  const works: string[] = [];
  for (let w = 0; w < workers; w++) {
    const wd = `${dir}/work-${w}`;
    while (!existsSync(`${wd}/.git`)) {
      const r = await git(["clone", "-q", REMOTE, wd], dir);
      if (r.code !== 0) { appendFileSync(`${dir}/log.txt`, `${now()} work clone failed: ${r.out.slice(-200)}\n`); rmSync(wd, { recursive: true, force: true }); await sleep(5000); }
    }
    works.push(wd);
  }
  const ops: Session["op"][] = ["clone", "fetch", "push"];
  const loop = async (w: number) => {
    await sleep(w * gapMs);
    while (Date.now() < state.endAt) {
      const t0 = Date.now();
      const n = ++state.n; save();
      const s = await session(run, n, w, ops[n % 3], works[w]);
      appendFileSync(`${dir}/sessions.jsonl`, JSON.stringify(s) + "\n");
      if (s.ok && s.front) last[s.front] = Date.now();
      await sleep(Math.max(0, gapMs * workers - (Date.now() - t0)));
    }
  };
  const killer = async () => {
    while (Date.now() < state.endAt) {
      if (Date.now() < state.nextKill) { await sleep(Math.min(5000, state.nextKill - Date.now())); continue; }
      // Kill the replica that served most recently, so each kill lands on a connector that is carrying sessions.
      const target = Object.entries(last).sort((a, b) => b[1] - a[1])[0]?.[0] ?? FRONTS[state.kills % 3];
      state.kills++; state.nextKill += KILL_EVERY_MS; save();
      let k: any;
      const before = Date.now();
      try { k = await harness("POST", `/front/${target}/kill`); } catch (e) { k = { error: String(e) }; }
      const rec: Record<string, unknown> = { kill: state.kills, target, ...k, at: new Date(k.at ?? Date.now()).toISOString() };
      // Times are on Cloudflare's clock (the orb's runs seconds behind): the restart from the front object's own event
      // log, and tunnel readiness from the new container's clock when its cloudflared /ready first reports a connection.
      const t0: number = k.at ?? Date.now();
      rec.orbClockOffsetMs = t0 - Math.round((before + Date.now()) / 2);
      for (let i = 0; i < 600; i++) {
        await sleep(500);
        try {
          if (rec.restartMs === undefined) {
            const st = await harness("GET", `/front/${target}/status`);
            const ev = (st.events as { at: number; kind: string }[]).filter((e) => e.kind === "start" && e.at > t0).sort((a, b) => a.at - b.at)[0];
            if (ev) { rec.restartMs = ev.at - t0; rec.colo = st.colo; }
            continue;
          }
          // busybox `date` has whole seconds only, so readiness is also timed on the orb's clock, shifted by the offset.
          const sent = Date.now();
          const sh = await harness("POST", `/front/${target}/status`, "curl -s --max-time 2 127.0.0.1:2000/ready; echo; date +%s");
          const [ready, at] = String(sh.out ?? "").split("\n");
          const n = /"readyConnections":\s*(\d+)/.exec(ready)?.[1];
          if (n && Number(n) > 0) {
            rec.tunnelReadyMs = Math.round((sent + Date.now()) / 2) + (rec.orbClockOffsetMs as number) - t0;
            rec.tunnelReadySecContainerClock = Number(at) - Math.floor(t0 / 1000);
            rec.readyConnections = Number(n); break;
          }
        } catch { /* keep polling */ }
      }
      appendFileSync(`${dir}/kills.jsonl`, JSON.stringify(rec) + "\n");
    }
  };
  await Promise.all([...Array.from({ length: workers }, (_, w) => loop(w)), killer()]);
  appendFileSync(`${dir}/log.txt`, `${now()} load end n=${state.n}\n`);
}

// ---- report ----

function jsonl<T>(p: string): T[] {
  return existsSync(p) ? readFileSync(p, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
}

async function report(): Promise<void> {
  const dir = `${RAW}/${opt.run}`;
  const ss = jsonl<Session>(`${dir}/sessions.jsonl`);
  const kills = jsonl<any>(`${dir}/kills.jsonl`);
  const ok = ss.filter((s) => s.ok);
  const pct = (a: number[], p: number) => { const x = [...a].sort((u, v) => u - v); return x[Math.min(x.length - 1, Math.floor(p * x.length))]; };
  const byOp: Record<string, object> = {};
  for (const op of ["clone", "fetch", "push"]) {
    const o = ss.filter((s) => s.op === op), g = o.filter((s) => s.ok).map((s) => s.ms);
    byOp[op] = { sessions: o.length, ok: g.length, p50: pct(g, 0.5), p99: pct(g, 0.99) };
  }
  const byFront: Record<string, number> = {};
  for (const s of ok) byFront[s.front!] = (byFront[s.front!] ?? 0) + 1;
  const causes: Record<string, number> = {};
  for (const s of ss.filter((x) => !x.ok)) {
    const e = s.err ?? "";
    const c = /wp7: session timeout/.test(e) ? "timeout (90 s)"
      : /Authenticated to|Authentication succeeded/.test(e) && /closed by remote host|Broken pipe|Connection closed|early EOF|hung up/.test(e) ? "stream closed after authentication"
      : /Connection closed by UNKNOWN|kex_exchange_identification|Connection closed by remote host/.test(e) ? "stream closed before the SSH banner"
      : /websocket: bad handshake|failed to connect to the origin|1033|502|530/.test(e) ? "edge: no healthy connector"
      : /Connection timed out during banner exchange/.test(e) ? "no SSH banner within ConnectTimeout"
      : /group refused|unreachable/.test(e) ? "write path (group.graft)" : /Permission denied/.test(e) ? "auth (tokens.graft)"
      : /closed by remote host|Broken pipe|early EOF|hung up/.test(e) ? "stream closed mid-session" : "other";
    causes[c] = (causes[c] ?? 0) + 1;
  }
  // Each failure attributed to the kill before it if it started within 2 minutes of that kill.
  // Session times are on the orb's clock and kill times on Cloudflare's; the first kill predates the offset measurement,
  // and its scheduled time on the orb's clock gives it: 4.46 s, less half a round trip.
  const killed = kills.map((k) => {
    const t = (typeof k.at === "number" ? k.at : Date.parse(k.at)) - (k.orbClockOffsetMs ?? 4400);
    const near = ss.filter((s) => { const st = Date.parse(s.start); return st + s.ms >= t && st <= t + 120_000; });
    const firstOkAfter = ss.filter((s) => s.ok && Date.parse(s.start) >= t).sort((a, b) => Date.parse(a.start) - Date.parse(b.start))[0];
    const firstOkSame = ss.filter((s) => s.ok && s.front === k.target && Date.parse(s.start) >= t).sort((a, b) => Date.parse(a.start) - Date.parse(b.start))[0];
    return { kill: k.kill, target: k.target, at: new Date(t + (k.orbClockOffsetMs ?? 4400)).toISOString(), restartMs: k.restartMs, tunnelReadyMs: k.tunnelReadyMs, colo: k.colo,
      failedWithin2Min: near.filter((s) => !s.ok).length, sessionsWithin2Min: near.length,
      firstOkSessionAfterMs: firstOkAfter ? Date.parse(firstOkAfter.start) + firstOkAfter.ms - t : null,
      firstOkOnKilledFrontAfterMs: firstOkSame ? Date.parse(firstOkSame.start) + firstOkSame.ms - t : null };
  });
  // Server-side check: every push the client saw acknowledged has its commit in the group log.
  const log: { seq: number; updates: string; front: string }[] = [];
  for (let since = 0; ;) { const page = await harness("GET", `/group/log?since=${since}`); if (!page.length) break; log.push(...page); since = page[page.length - 1].seq; }
  const logged = new Set(log.map((l) => l.updates.split(" ")[1]));
  const pushes = ok.filter((s) => s.op === "push");
  const summary = {
    run: opt.run, first: ss[0]?.start, last: ss[ss.length - 1]?.start,
    hours: ss.length ? (Date.parse(ss[ss.length - 1].start) - Date.parse(ss[0].start)) / 3600_000 : 0,
    sessions: ss.length, ok: ok.length, successRate: ss.length ? ok.length / ss.length : 0, byOp, byFront, failureCauses: causes,
    pushesAcked: pushes.length, pushesInGroupLog: pushes.filter((s) => s.sha && logged.has(s.sha)).length, groupLogEntries: log.length,
    kills: killed,
    failures: ss.filter((s) => !s.ok).map((s) => ({ n: s.n, op: s.op, start: s.start, ms: s.ms, err: s.err })),
  };
  writeFileSync(`${dir}/summary.json`, JSON.stringify(summary, null, 1));
  const { failures, ...head } = summary;
  console.log(JSON.stringify(head, null, 1), `\nfailures: ${failures.length}`);
}

async function del(): Promise<void> {
  for (const f of FRONTS) { try { await harness("POST", `/front/${f}/stop`); } catch (e) { console.error(f, String(e)); } }
  try { execFileSync("npx", ["wrangler", "delete", "--name", "graft-m0-ssh", "--force"], { cwd: ROOT, stdio: "inherit" }); } catch (e) { console.error(String(e)); }
  const tid = await tunnelId().catch(() => null);
  const routes = await cf("GET", `/accounts/${ACCOUNT}/teamnet/routes?comment=M0-WP7%20sshd`);
  for (const r of routes.result ?? []) console.log("route", (await cf("DELETE", `/accounts/${ACCOUNT}/teamnet/routes/${r.id}`)).success);
  const apps = await cf("GET", `/zones/${ZONE}/spectrum/apps`);
  for (const a of apps.result ?? []) if (/^ssh\.graftyard\.dev$/.test(a.dns?.name)) console.log("spectrum", (await cf("DELETE", `/zones/${ZONE}/spectrum/apps/${a.id}`)).success);
  const dns = await cf("GET", `/zones/${ZONE}/dns_records?name=${HOST}`);
  for (const r of dns.result ?? []) console.log("dns", (await cf("DELETE", `/zones/${ZONE}/dns_records/${r.id}`)).success);
  if (tid) {
    await cf("DELETE", `/accounts/${ACCOUNT}/cfd_tunnel/${tid}/connections`);
    console.log("tunnel", (await cf("DELETE", `/accounts/${ACCOUNT}/cfd_tunnel/${tid}`)).success);
  }
  const vn = await cf("GET", `/accounts/${ACCOUNT}/teamnet/virtual_networks?name=${NAMES.vnet}`);
  for (const v of vn.result ?? []) console.log("vnet", (await cf("DELETE", `/accounts/${ACCOUNT}/teamnet/virtual_networks/${v.id}`)).success);
}

const cmd = positionals[0];
if (cmd === "spectrum") await spectrum();
else if (cmd === "deploy") await deploy();
else if (cmd === "status") console.log(JSON.stringify(await status(), null, 1));
else if (cmd === "load") await load();
else if (cmd === "report") await report();
else if (cmd === "stop") { for (const f of FRONTS) console.log(f, JSON.stringify(await harness("POST", `/front/${f}/stop`))); }
else if (cmd === "delete") await del();
else { console.error("usage: node run/wp7.ts spectrum|deploy|status|load --run X|report --run X|stop|delete"); process.exit(2); }
