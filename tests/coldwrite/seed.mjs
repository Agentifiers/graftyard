// coldwrite's seed (`run.sh`): through the bed's front (`driver.mjs`), the system groups a push reads are made with
// `/mk`, then one owner, its login and `Placement` row, its `Authz` chain opened, a repository with its `RepoOwner`
// row, the install account's `Placement` row and opened `Authz` (as genesis acceptance makes them, `after_genesis`),
// and one stored PAT for the owner, all committed through the edge's own `/_bed/identity` as the operator.
// Writes <out>/seed.json: the token (0600 file, never printed), the login, and the repository name.
//   node tests/coldwrite/seed.mjs <front url> <bed token> <out dir>
import { createHash, randomBytes } from "node:crypto";
import { writeFileSync } from "node:fs";

const [FRONT, BED, OUT] = process.argv.slice(2);
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
const OWNER = 7000001, INSTALL = 1, LOGIN = "cw", REPO = "r";

const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const shard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id) => shard("graftyard/names/account/", be64(id));
const loginShard = (l) => shard("graftyard/names/login/", Buffer.from(l.toLowerCase()));
const repoShard = (r) => shard("graftyard/names/repo/", be64(r));

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

async function commit(chain, op, ...allowed) {
  const r = await fetch(`${FRONT}/_bed/identity`, {
    method: "POST", headers: { "x-bed-token": BED }, body: JSON.stringify({ chain, cmd: { cred: OP, op } }),
  });
  const v = await r.json();
  const err = v.outcome?.refused?.err ?? v.outcome?.error;
  if (!v.outcome?.ok && !allowed.some((a) => JSON.stringify(v.outcome ?? {}).includes(a))) {
    throw new Error(`${chain} ${Object.keys(op)[0]}: ${JSON.stringify(v).slice(0, 400)} ${err ?? ""}`);
  }
  return v.outcome;
}

const tok = token();
for (const c of new Set(["ids", `names:${accountShard(OWNER)}`, `names:${accountShard(INSTALL)}`, `names:${loginShard(LOGIN)}`, `tokens:${tok.shard}`])) {
  await mk(c);
}
await commit(`names:${accountShard(OWNER)}`, { PutAccount: { id: OWNER, suspended: false, sessions_not_before: 0 } });
await commit(`names:${accountShard(OWNER)}`, { InitPlacement: { owner: OWNER, record: TRI } }, "RecordExists");
await commit(`names:${loginShard(LOGIN)}`, { ClaimLogin: { login: LOGIN, kind: "User", account: OWNER } });
await commit(`authz:${OWNER}`, { Open: { kind: "User", enterprise: null } });
const made = await commit(`authz:${OWNER}`, { CreateRepo: { name: REPO } });
const repo = made?.ok?.out?.Repo?.repo;
if (!repo) throw new Error(`CreateRepo: ${JSON.stringify(made)}`);
await mk(`names:${repoShard(repo)}`);
await commit(`names:${repoShard(repo)}`, { RepoOwner: { repo, owner: OWNER } });
await commit(`names:${accountShard(INSTALL)}`, { InitPlacement: { owner: INSTALL, record: TRI } }, "RecordExists");
await commit(`authz:${INSTALL}`, { Open: { kind: "Org", enterprise: null } });
await commit(`tokens:${tok.shard}`, { Store: { hash: tok.hash, kind: "Pat", account: OWNER, scopes: 1, expires: null } });
writeFileSync(`${OUT}/seed.json`, JSON.stringify({ token: tok.text, login: LOGIN, repo: REPO, id: repo }), { mode: 0o600 });
console.log(`seeded ${LOGIN}/${REPO} (repo ${repo}), install account ${INSTALL}`);
