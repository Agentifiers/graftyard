// plaindiff's second seed (`run.sh`), after coldwrite's: both accounts' `AccountLogin` rows, which a node id and a
// login render from; a read-only collaborator `cw2` with its own PAT, granted Read on cw/r on the owner's `Authz` chain
// through the edge's `/_bed/identity`; and an issue with one comment by the owner and the label `plain`, made through
// edge 0 with the owner's token. `diff.mjs` sends `cw2`'s deletes of the comment and the label to both edges: each is
// refused in `decide` (below write), so the pair shows what a refusal renders. Writes <bed>/seed2.json (0600, never
// printed): the reader's token and the comment's id.
//   node tests/plaindiff/seed2.mjs <front url> <bed dir>
import { createHash, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const [FRONT, B] = process.argv.slice(2);
const seed = JSON.parse(readFileSync(`${B}/seed.json`, "utf8"));
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
const OWNER = 7000001, READER = 7000002, LOGIN = "cw2";

const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const shard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id) => shard("graftyard/names/account/", be64(id));
const loginShard = (l) => shard("graftyard/names/login/", Buffer.from(l.toLowerCase()));

function token() {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  const secret = Array.from(randomBytes(30), (b) => A[b % 62]).join("");
  let crc = 0xffffffff;
  for (const c of Buffer.from(secret)) { crc ^= c; for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1)); }
  let v = (crc ^ 0xffffffff) >>> 0;
  const tail = Array(6).fill("0");
  for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
  return { text: `ghp_${secret}${tail.join("")}`, hash: [...h], shard: h.readUInt32BE(0) % 3072 };
}

async function mk(chain) {
  const r = await fetch(`${FRONT}/mk?chain=${chain}&record=${encodeURIComponent(JSON.stringify(TRI))}`);
  const v = await r.json();
  if (v.role !== "leader") throw new Error(`mk ${chain}: ${JSON.stringify(v)}`);
}

async function commit(chain, op) {
  const r = await fetch(`${FRONT}/_bed/identity`, {
    method: "POST", headers: { "x-bed-token": "cw-bed" }, body: JSON.stringify({ chain, cmd: { cred: OP, op } }),
  });
  const v = await r.json();
  if (!v.outcome?.ok) throw new Error(`${chain} ${Object.keys(op)[0]}: ${JSON.stringify(v).slice(0, 400)}`);
}

const owner = { authorization: `token ${seed.token}`, "user-agent": "gh/2.102.1", "content-type": "application/json", "x-cw-edge": "0" };
async function api(method, path, body) {
  const r = await fetch(`${FRONT}${path}`, { method, headers: owner, body: body && JSON.stringify(body) });
  const v = await r.json();
  if (r.status >= 300 || v.errors) throw new Error(`${method} ${path}: ${r.status} ${JSON.stringify(v).slice(0, 300)}`);
  return v;
}
const gql = (query, variables) => api("POST", "/api/graphql", { query, variables }).then((v) => v.data);

const tok = token();
for (const c of new Set([`names:${accountShard(READER)}`, `names:${loginShard(LOGIN)}`, `tokens:${tok.shard}`])) await mk(c);
await commit(`names:${accountShard(READER)}`, { PutAccount: { id: READER, suspended: false, sessions_not_before: 0 } });
await commit(`names:${loginShard(LOGIN)}`, { ClaimLogin: { login: LOGIN, kind: "User", account: READER } });
// The owner's and the reader's `AccountLogin` rows (F50), which coldwrite's seed does not write: a node id resolves
// its repository's owner by them, and an account's login renders from them.
await commit(`names:${accountShard(OWNER)}`, { SetAccountLogin: { account: OWNER, login: seed.login } });
await commit(`names:${accountShard(READER)}`, { SetAccountLogin: { account: READER, login: LOGIN } });
await commit(`authz:${OWNER}`, { SetGrant: { repo: seed.id, user: READER, permission: "Read" } });
await commit(`tokens:${tok.shard}`, { Store: { hash: tok.hash, kind: "Pat", account: READER, scopes: 1, expires: null } });

const { repository } = await gql(`{ repository(owner: "${seed.login}", name: "${seed.repo}") { id } }`);
const { createIssue } = await gql("mutation($r: ID!) { createIssue(input: { repositoryId: $r, title: \"plain\" }) { issue { id number } } }", { r: repository.id });
await gql("mutation($s: ID!) { addComment(input: { subjectId: $s, body: \"by the owner\" }) { clientMutationId } }", { s: createIssue.issue.id });
const comments = await api("GET", `/api/v3/repos/${seed.login}/${seed.repo}/issues/${createIssue.issue.number}/comments`);
await api("POST", `/api/v3/repos/${seed.login}/${seed.repo}/labels`, { name: "plain", color: "ededed" });
writeFileSync(`${B}/seed2.json`, JSON.stringify({ token: tok.text, comment: comments[0].id }), { mode: 0o600 });
console.log(`seeded reader ${LOGIN}, issue #${createIssue.issue.number} with comment ${comments[0].id}, label plain`);
