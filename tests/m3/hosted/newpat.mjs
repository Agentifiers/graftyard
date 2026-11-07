// Prints a fresh `ghp_` for <who> (member or admin) of the setup, stored on its `Tokens` shard and listed on the
// account's `Identity` chain through the tools edge and the edge's bed route, as the operator (stdout goes to a 0600
// file).
//   node tests/m3/hosted/newpat.mjs <state dir> <who>
import { createHash, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
const [dir, who] = process.argv.slice(2);
const s = JSON.parse(readFileSync(`${dir}/m3.json`, "utf8"));
const acct = who === "admin" ? s.admin : s.member;
const ROOT = new URL("./", import.meta.url).pathname;
const TOOLS = process.env.GRAFT_TOOLS_EDGE ?? "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev";
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const rid = () => { const C = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"; let v = BigInt(Date.now()) & 0xffff_ffff_ffffn; for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b); let o = ""; for (let i = 25; i >= 0; i--) o += C[Number((v >> BigInt(5 * i)) & 31n)]; return o; };
let secret = "";
while (secret.length < 30) for (const b of randomBytes(32)) if (b < 248 && secret.length < 30) secret += A[b % 62];
let c = ~0; for (const ch of Buffer.from(secret)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
let v = (~c) >>> 0; const tail = Array(6).fill("0"); for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
const shard = h.readUInt32BE(0) % 3072;
const post = async (path, body) => (await fetch(TOOLS + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` } })).json();
const chain = `tokens:${shard}`;
await post("/create", { chain });
const o = await post("/commit", { chain, rid: rid(), cmd: { cred: { account: null, key: null }, op: { Store: { hash: [...h], kind: "Pat", account: acct, scopes: 1, expires: null } } } });
if (!o.outcome?.ok) throw new Error(`Store: ${JSON.stringify(o).slice(0, 300)}`);
process.stdout.write(`ghp_${secret}${tail.join("")}`);
