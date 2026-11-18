// Rotates graft-m2b's seeded owner, writer and reader tokens (~/.graft-m2b/tokens.txt lines 1-3; line 4, revoked by
// the seed, is kept): each old token is revoked through the install's own `DELETE /applications/{client}/token` (as
// gh's public client, token in the JSON body only), a fresh `ghp_` is stored for the same account on its `Tokens`
// shard through the tools edge as the operator (as tests/m1/integ/seed.ts does), and tokens.txt is rewritten (0600,
// via a rename). The account of each line is read from its stored row before the revoke. Prints statuses only; no
// part of any token is printed.
//   node tests/am1/runs/rotate.mjs
import { createHash, randomBytes } from "node:crypto";
import { readFileSync, renameSync, writeFileSync } from "node:fs";

const FILE = `${process.env.HOME}/.graft-m2b/tokens.txt`;
const EDGE = `https://graft-m2b-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOOLS = `https://graft-m2b-tools-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const OPTOK = readFileSync(`${process.env.GRAFT_MAIN_CHECKOUT ?? "/home/user/workspace/repo"}/tests/m0/.m0-token`, "utf8").trim();
const GH = "178c6fc778ccc68e1d6a";
const ACCTS = [7000001, 7000002, 7000003];
const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const rid = () => { const C = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; let v = BigInt(Date.now()) & 0xffff_ffff_ffffn; for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b); let o = ""; for (let i = 25; i >= 0; i--) o += C[Number((v >> BigInt(5 * i)) & 31n)]; return o; };
const tools = async (path, body) => (await fetch(TOOLS + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${OPTOK}` } })).json();

function mint() {
  let secret = "";
  while (secret.length < 30) for (const b of randomBytes(32)) if (b < 248 && secret.length < 30) secret += A[b % 62];
  let c = ~0; for (const ch of Buffer.from(secret)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
  let v = (~c) >>> 0; const tail = Array(6).fill("0"); for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
  return { text: `ghp_${secret}${tail.join("")}`, hash: h, shard: h.readUInt32BE(0) % 3072 };
}
const user = async (t) => { const r = await fetch(`${EDGE}/api/v3/user`, { headers: { authorization: `token ${t}` } }); const j = await r.json().catch(() => ({})); return `${r.status}${j.login ? ` (${j.login} ${j.id})` : ""}`; };

const lines = readFileSync(FILE, "utf8").trim().split("\n");
if (lines.length !== 4) throw new Error(`expected 4 lines, got ${lines.length}`);
const out = [...lines];
for (let i = 0; i < 3; i++) {
  const [old, hex] = lines[i].split(" ");
  const shard = Buffer.from(hex, "hex").readUInt32BE(0) % 3072;
  const before = await user(old);
  const del = await fetch(`${EDGE}/api/v3/applications/${GH}/token`, {
    method: "DELETE",
    headers: { authorization: `Basic ${Buffer.from(`${GH}:x`).toString("base64")}`, "content-type": "application/json" },
    body: JSON.stringify({ access_token: old }),
  });
  const t = mint();
  await tools("/create", { chain: `tokens:${t.shard}` });
  const o = await tools("/commit", { chain: `tokens:${t.shard}`, rid: rid(), cmd: { cred: { account: null, key: null }, op: { Store: { hash: [...t.hash], kind: "Pat", account: ACCTS[i], scopes: 1, expires: null } } } });
  if (!o.outcome?.ok) throw new Error(`Store line ${i + 1}: refused or failed`);
  out[i] = `${t.text} ${t.hash.toString("hex")}`;
  writeFileSync(`${FILE}.new`, out.join("\n") + "\n", { mode: 0o600 });
  renameSync(`${FILE}.new`, FILE);
  console.log(`line ${i + 1} account ${ACCTS[i]}: old tokens:${shard} GET /user before=${before}; revoke=${del.status}; new stored on tokens:${t.shard}`);
}
// The revocation is read back after the edge's 10 s token cache.
await new Promise((r) => setTimeout(r, 11_000));
for (let i = 0; i < 3; i++) {
  const [old] = lines[i].split(" ");
  const [neu] = out[i].split(" ");
  console.log(`line ${i + 1}: old GET /user=${await user(old)} new GET /user=${await user(neu)}`);
}
