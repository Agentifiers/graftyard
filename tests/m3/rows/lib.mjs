// Shared by the tests/m3/rows drivers on graft-m2b (source tests/m3/rows/env.sh first): the edge, the tools edge as
// the operator, the edge's bed route for owner chains, token and key helpers, and `scan`, which refuses to write a
// raw log line that holds a token, a key, or a secret. No secret is put on argv, in a URL, or in a log.
import { createHash, generateKeyPairSync, randomBytes } from "node:crypto";
import { appendFileSync, readFileSync } from "node:fs";

export const EDGE = process.env.GRAFT_EDGE;
export const TOOLS = process.env.GRAFT_TOOLS_EDGE;
export const STATE = process.env.M3_STATE;
const ROOT = new URL("./", import.meta.url).pathname;
const TOOLS_TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
export const BED = readFileSync(process.env.BED_TOKEN_FILE, "utf8").trim();
export const OP = { account: null, key: null };
export const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
export const INSTALL = 1;
export const state = JSON.parse(readFileSync(`${STATE}/m3.json`, "utf8"));
export const tokens = Object.fromEntries(readFileSync(`${STATE}/m3-tokens.txt`, "utf8").trim().split("\n").map((l) => l.split(" ")));
export const log = (m) => console.log(`${new Date().toISOString()} ${m}`);
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const as = (a) => ({ account: a, key: null });

/** Every secret this run made or read, so `scan` can refuse a line that holds one. */
const secrets = new Set([BED, TOOLS_TOKEN, ...Object.values(tokens)]);
export const secret = (s) => { if (s && s.length >= 8) secrets.add(s); return s; };
const SHAPES = /(gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9]{36}|ssh-ed25519 [A-Za-z0-9+/=]{40,}|eyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]{20,})/;
export function scan(line) {
  for (const s of secrets) if (line.includes(s)) throw new Error("a secret reached a log line");
  if (SHAPES.test(line)) throw new Error("a token-shaped string reached a log line");
  return line;
}
export const row = (file, v) => appendFileSync(file, scan(JSON.stringify(v)) + "\n");

async function post(url, body, headers) {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(url, { method: "POST", body: JSON.stringify(body), headers, signal: AbortSignal.timeout(300_000) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 200)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      await sleep(2000 * (i + 1));
    }
  }
}
export const tools = (path, body) => post(TOOLS + path, body, { authorization: `Bearer ${TOOLS_TOKEN}` });
export const bed = (body) => post(`${EDGE}/_bed/identity`, body, { "x-bed-token": BED });

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
export async function read(chain, q) {
  if (ownerChain(chain)) return (await bed({ chain, read: q })).value;
  return (await tools("/read", { chain, q })).value;
}
export const refusal = (o) => o?.refused?.err?.split?.(/[ ({]/)[0] ?? (o?.ok ? "ok" : JSON.stringify(o).slice(0, 120));
export async function must(chain, cmd, ...allowed) {
  const o = await commit(chain, cmd);
  if (!o?.ok && !allowed.includes(refusal(o))) throw new Error(`${chain}: ${refusal(o)}`);
  return o;
}
const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const nshard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
export const accountShard = (id) => nshard("graftyard/names/account/", be64(id));
export const loginShard = (l) => nshard("graftyard/names/login/", Buffer.from(l.toLowerCase()));
export const repoShard = (r) => nshard("graftyard/names/repo/", be64(r));

/** A user with its login, `AccountLogin`, `Account`, `Placement`, opened `Authz`, and `Identity` rows. */
export async function user(a, login, kind = "User") {
  await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind, account: a } } }, "Taken");
  await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
  await must(`authz:${a}`, { cred: OP, op: { Open: { kind, enterprise: null } } }, "AlreadyOpened");
  if (kind === "User") await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind, created: Date.now() } } }, "AlreadyCreated");
  return a;
}

const B62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/** A new classic PAT for `account`, stored as the operator and listed on its `Identity` chain. */
export async function pat(account, title = "rows PAT") {
  let s = "";
  while (s.length < 30) for (const b of randomBytes(32)) if (b < 248 && s.length < 30) s += B62[b % 62];
  let c = ~0; for (const ch of Buffer.from(s)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
  let v = (~c) >>> 0; const tail = Array(6).fill("0"); for (let i = 5; i >= 0; i--) { tail[i] = B62[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(s).digest();
  await must(`tokens:${h.readUInt32BE(0) % 3072}`, { cred: OP, op: { Store: { hash: [...h], kind: "Pat", account, scopes: 1, expires: null } } });
  await must(`identity:${account}`, { cred: OP, op: { AddCredential: { credential: { Token: { hash: [...h], kind: "Pat" } }, title } } });
  return { text: secret(`ghp_${s}${tail.join("")}`), hash: h.toString("hex") };
}

/** A fresh ed25519 public key line and its OpenSSH SHA-256 fingerprint (hex and `SHA256:` base64). */
export function sshKey() {
  const { publicKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  const ty = Buffer.from("ssh-ed25519");
  const len = (b) => { const l = Buffer.alloc(4); l.writeUInt32BE(b.length); return l; };
  const blob = Buffer.concat([len(ty), ty, len(raw), raw]);
  const fp = createHash("sha256").update(blob).digest();
  const line = `ssh-ed25519 ${blob.toString("base64")}`;
  secret(line);
  return { line, fp: fp.toString("hex"), sha256: `SHA256:${fp.toString("base64").replace(/=+$/, "")}` };
}

/** A REST call to the edge under `token`. The token rides in the header only. */
export async function api(method, path, token, body, extra = {}) {
  const headers = { accept: "application/vnd.github+json", ...extra };
  if (token) headers.authorization = `token ${token}`;
  if (body !== undefined) headers["content-type"] = "application/json";
  const r = await fetch(`${EDGE}/api/v3${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000) });
  const t = await r.text();
  let b = null;
  try { b = t ? JSON.parse(t) : null; } catch { b = t.slice(0, 200); }
  return { status: r.status, body: b };
}
