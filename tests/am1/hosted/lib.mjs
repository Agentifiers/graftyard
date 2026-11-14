// Shared by the AM1 hosted drivers on graft-m1: the edge, the tools edge's raw member calls, the m3-ci App's JWT and
// installation tokens (with `graftyard_task`), PATs, git through a credential helper, and `scan`, which refuses to
// write a log line that holds a token. No secret goes on argv, in a URL, or in a log.
import { createHash, createSign, randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import { appendFileSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const EDGE = process.env.GRAFT_EDGE ?? "https://graft-m1-edge.<workers-subdomain>.workers.dev";
export const HOST = EDGE.split("//")[1];
export const TOOLS = process.env.GRAFT_TOOLS_EDGE ?? "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev";
const MAIN = process.env.GRAFT_MAIN_CHECKOUT ?? "/home/user/workspace/repo";
const M3H = process.env.M3_STATE ?? `${process.env.HOME}/.graft-m3h`;
const TOOLS_TOKEN = readFileSync(process.env.TOOLS_TOKEN_FILE ?? `${MAIN}/tests/m0/.m0-token`, "utf8").trim();
export const BED = readFileSync(process.env.BED_TOKEN_FILE ?? `${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
export const state = JSON.parse(readFileSync(`${M3H}/m3.json`, "utf8"));
export const tokens = Object.fromEntries(readFileSync(`${M3H}/m3-tokens.txt`, "utf8").trim().split("\n").map((l) => l.split(" ")));
const PEM = readFileSync(`${M3H}/app.pem`, "utf8");
export const OP = { account: null, key: null };
export const log = (m) => console.log(scan(`${new Date().toISOString()} ${m}`));
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const secrets = new Set([BED, TOOLS_TOKEN, ...Object.values(tokens)]);
export const secret = (s) => { if (s && s.length >= 8) secrets.add(s); return s; };
const SHAPES = /(gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9]{36}|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,})/;
export function scan(line) {
  for (const s of secrets) if (line.includes(s)) throw new Error("a secret reached a log line");
  if (SHAPES.test(line)) throw new Error("a token-shaped string reached a log line");
  return line;
}
export const row = (file, v) => appendFileSync(file, scan(JSON.stringify(v)) + "\n");

/** A request to the edge under `token` (header only). `path` is absolute (`/api/v3/...` or `/api/graftyard/v1/...`). */
export async function api(method, path, token, body, extra = {}) {
  const headers = { accept: "application/vnd.github+json", "user-agent": "graftyard-am1", ...extra };
  if (token) headers.authorization = `token ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  const t0 = Date.now();
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(`${EDGE}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000) });
      const t = await r.text();
      let b = null;
      try { b = t ? JSON.parse(t) : null; } catch { b = t.slice(0, 300); }
      return { status: r.status, body: b, headers: Object.fromEntries(r.headers), ms: Date.now() - t0 };
    } catch (e) {
      if (i >= 2) return { status: 0, body: String(e).slice(0, 200), headers: {}, ms: Date.now() - t0 };
      await sleep(1000);
    }
  }
}

async function post(url, body, headers, timeout = 120_000) {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(url, { method: "POST", body: JSON.stringify(body), headers: { "content-type": "application/json", "user-agent": "graftyard-am1", ...headers }, signal: AbortSignal.timeout(timeout) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 200)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 3) throw e;
      await sleep(1500 * (i + 1));
    }
  }
}
/** One raw call to a member through the tools edge: `{status, v}`. */
export const member = (chain, site, path, body, deadline = 30_000, doName = undefined) =>
  post(`${TOOLS}/member`, { chain, site, path, body, deadline, gen: 1, do_name: doName }, { authorization: `Bearer ${TOOLS_TOKEN}` }, deadline + 30_000);
export const tools = (path, body) => post(TOOLS + path, body, { authorization: `Bearer ${TOOLS_TOKEN}` });
export const bed = (body) => post(`${EDGE}/_bed/identity`, body, { "x-bed-token": BED });
export async function bedGet(path) {
  const r = await fetch(`${EDGE}${path}`, { method: "POST", headers: { "x-bed-token": BED }, signal: AbortSignal.timeout(120_000) });
  return { status: r.status, body: await r.text() };
}

/** Every member's `/status` of a chain (sites 0..n-1). */
export async function statuses(chain, n = 3) {
  return Promise.all([...Array(n).keys()].map(async (s) => {
    try { return { site: s, ...(await member(chain, s, "/status", undefined, 15_000)).v }; } catch (e) { return { site: s, error: String(e).slice(0, 100) }; }
  }));
}
/** Each site's current voter object of a chain, from a member's committed config: a voter replaced since the group
 * was made has a later generation's name, and a call by site alone reaches the first generation's object. */
export async function voterNames(chain) {
  for (const st of await statuses(chain)) {
    const vs = st.config?.config?.normal?.voters;
    if (st.role === "leader" && vs) return Object.fromEntries(vs.map((v) => [v.site, v.do_name]));
  }
  return {};
}
export async function leaderOf(chain) {
  for (let i = 0; i < 20; i++) {
    const st = await statuses(chain);
    const l = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
    if (l) return l.site;
    await sleep(1000);
  }
  throw new Error(`no leader for ${chain}`);
}
/** The leader's stored log from `from` with the family's fields (test-faults `/log`). */
export async function logFrom(chain, from, n = 500) {
  const s = Number(process.env.LOG_SITE ?? await leaderOf(chain));
  const r = await fetch(`${TOOLS}/member`, { method: "POST", headers: { authorization: `Bearer ${TOOLS_TOKEN}`, "content-type": "application/json", "user-agent": "graftyard-am1" },
    body: JSON.stringify({ chain, site: s, path: `/log?from=${from}&n=${n}`, deadline: 30_000, gen: 1 }) });
  return (await r.json()).v ?? [];
}

// ---- the m3-ci App ----

const b64u = (b) => Buffer.from(b).toString("base64url");
export function appJwt(app = state.app) {
  const now = Math.floor(Date.now() / 1000);
  const h = b64u(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const p = b64u(JSON.stringify({ iat: now - 30, exp: now + 540, iss: String(app) }));
  const s = createSign("RSA-SHA256").update(`${h}.${p}`).sign(PEM);
  return secret(`${h}.${p}.${b64u(s)}`);
}
/** An installation token for m3-ci on m3org; `extra` adds `repositories`, `graftyard_task`, `permissions`. */
export async function ghs(extra = {}) {
  const r = await api("POST", `/api/v3/app/installations/${state.installation}/access_tokens`, null, extra, { authorization: `Bearer ${appJwt()}` });
  if (r.status === 201) secret(r.body.token);
  return r;
}

const B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
export const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const nshard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
export const accountShard = (id) => nshard("graftyard/names/account/", be64(id));
export const loginShard = (l) => nshard("graftyard/names/login/", Buffer.from(l.toLowerCase()));
export const repoShard = (r) => nshard("graftyard/names/repo/", be64(r));
export function rid() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}
const ownerChain = (chain) => chain.startsWith("authz:") || chain.startsWith("identity:");
export async function commit(chain, cmd) {
  if (ownerChain(chain)) return (await bed({ chain, cmd })).outcome;
  await tools("/create", { chain });
  return (await tools("/commit", { chain, cmd, rid: rid() })).outcome;
}
export const refusal = (o) => o?.refused?.err?.split?.(/[ ({]/)[0] ?? (o?.ok ? "ok" : JSON.stringify(o).slice(0, 120));
export async function must(chain, cmd, ...allowed) {
  const o = await commit(chain, cmd);
  if (!o?.ok && !allowed.includes(refusal(o))) throw new Error(`${chain}: ${refusal(o)}`);
  return o;
}
/** A new classic PAT for `account`, stored as the operator. */
export async function pat(account, title = "am1 PAT") {
  let s = "";
  while (s.length < 30) for (const b of randomBytes(32)) if (b < 248 && s.length < 30) s += B62[b % 62];
  let c = ~0; for (const ch of Buffer.from(s)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
  let v = (~c) >>> 0; const tail = Array(6).fill("0"); for (let i = 5; i >= 0; i--) { tail[i] = B62[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(s).digest();
  await must(`tokens:${h.readUInt32BE(0) % 3072}`, { cred: OP, op: { Store: { hash: [...h], kind: "Pat", account, scopes: 1, expires: null } } });
  await must(`identity:${account}`, { cred: OP, op: { AddCredential: { credential: { Token: { hash: [...h], kind: "Pat" } }, title } } });
  return secret(`ghp_${s}${tail.join("")}`);
}

// ---- git with a credential helper reading a 0600 file ----

export class Git {
  constructor(repoPath) {
    this.dir = mkdtempSync(join(tmpdir(), "am1-git-"));
    this.tok = join(this.dir, ".tok");
    writeFileSync(join(this.dir, ".cred.sh"), `#!/bin/sh\n[ "$1" = get ] || exit 0\necho username=x\necho "password=$(cat "${this.tok}")"\n`, { mode: 0o700 });
    this.url = `${EDGE}/${repoPath}.git`;
    this.wt = join(this.dir, "wt");
  }
  as(token) { writeFileSync(this.tok, token, { mode: 0o600 }); return this; }
  run(args, cwd = this.wt) {
    try {
      const out = execFileSync("git", ["-c", "credential.helper=", "-c", `credential.helper=${join(this.dir, ".cred.sh")}`,
        "-c", "user.name=am1", "-c", "user.email=am1@x", ...args], { cwd, env: { ...process.env, GIT_TERMINAL_PROMPT: "0" }, stdio: ["ignore", "pipe", "pipe"], timeout: 300_000 });
      return { ok: true, out: out.toString() };
    } catch (e) {
      return { ok: false, out: `${e.stdout ?? ""}${e.stderr ?? ""}`.slice(0, 2000) };
    }
  }
  clone() { return this.run(["clone", "-q", this.url, this.wt], this.dir); }
  remoteHead(ref) { const r = this.run(["ls-remote", this.url, ref], this.dir); return r.ok ? r.out.split("\t")[0] : null; }
}
