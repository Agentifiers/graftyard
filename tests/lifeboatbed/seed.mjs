// lifeboatbed's seed (`run.sh`): through the install's front, on an install whose `Release` group holds a real genesis
// (so `after_genesis` made the install account's rows), one owner with its login (both rows), `Placement`, opened `Authz` chain, a
// repository with its `RepoOwner` row, and a stored PAT, all committed through the edge's own `/_bed/identity` as the
// operator, each system group created on first use (`"create": true`). Then two pushes through `/_bed/push`, each
// placing a stored pack (`stored`); `run.sh` opens the pull request with gh.
// Writes <out>/seed.json (0600): the PAT, the login, the repository and its id, and the packs.
// `push` mode pushes one more stored pack to `f` with the seeded PAT and appends it to seed.json's packs.
//   node tests/lifeboatbed/seed.mjs [push] <front url> <bed token> <out dir>
import { createHash, randomBytes } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const MODE = process.argv[2] === "push" ? "push" : "seed";
const [FRONT, BED, OUT] = process.argv.slice(MODE === "push" ? 3 : 2);
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
const OWNER = 7000001, LOGIN = "lb", REPO = "r";

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

async function commit(chain, op, ...allowed) {
  const r = await fetch(`${FRONT}/_bed/identity`, {
    method: "POST", headers: { "x-bed-token": BED },
    body: JSON.stringify({ chain, create: chain.startsWith("names:") || chain.startsWith("tokens:"), cmd: { cred: OP, op } }),
  });
  const t = await r.text();
  let v; try { v = JSON.parse(t); } catch { throw new Error(`${chain} ${Object.keys(op)[0]}: ${r.status} ${t.slice(0, 400)}`); }
  if (!v.outcome?.ok && !allowed.some((a) => JSON.stringify(v.outcome ?? {}).includes(a))) {
    throw new Error(`${chain} ${Object.keys(op)[0]}: ${JSON.stringify(v).slice(0, 400)}`);
  }
  return v.outcome;
}

const hex40 = () => randomBytes(20).toString("hex");
async function push(tok, repo, ref, expect, stored) {
  const r = await fetch(`${FRONT}/_bed/push?repo=${repo}`, {
    method: "POST", headers: { "x-bed-token": BED, authorization: `token ${tok}` },
    body: JSON.stringify({ ref, expect, new: hex40(), stored }),
  });
  const v = await r.json();
  const o = v.outcome?.Ok ?? v.outcome;
  if (!o?.ok) throw new Error(`push ${ref}: ${JSON.stringify(v).slice(0, 400)}`);
  return v.pack;
}

if (MODE === "push") {
  const s = JSON.parse(readFileSync(`${OUT}/seed.json`));
  s.packs.push(await push(readFileSync(`${OUT}/pat`, "utf8"), s.id, `refs/heads/g${s.packs.length}`, null, 110_000));
  writeFileSync(`${OUT}/seed.json`, JSON.stringify(s), { mode: 0o600 });
  console.log(`pushed pack ${s.packs.length}`);
  process.exit(0);
}

const tok = token();
await commit(`names:${accountShard(OWNER)}`, { PutAccount: { id: OWNER, suspended: false, sessions_not_before: 0 } });
await commit(`names:${accountShard(OWNER)}`, { InitPlacement: { owner: OWNER, record: TRI } }, "RecordExists");
await commit(`names:${loginShard(LOGIN)}`, { ClaimLogin: { login: LOGIN, kind: "User", account: OWNER } }, "Taken");
await commit(`names:${accountShard(OWNER)}`, { SetAccountLogin: { account: OWNER, login: LOGIN } });
await commit(`authz:${OWNER}`, { Open: { kind: "User", enterprise: null } }, "AlreadyOpened");
const made = await commit(`authz:${OWNER}`, { CreateRepo: { name: REPO } });
const repo = made?.ok?.out?.Repo?.repo;
if (!repo) throw new Error(`CreateRepo: ${JSON.stringify(made)}`);
await commit(`names:${repoShard(repo)}`, { RepoOwner: { repo, owner: OWNER } });
await commit(`tokens:${tok.shard}`, { Store: { hash: tok.hash, kind: "Pat", account: OWNER, scopes: 1, expires: null } });
writeFileSync(`${OUT}/pat`, tok.text, { mode: 0o600 });
const packs = [];
packs.push(await push(tok.text, repo, "refs/heads/main", null, 70_000));
packs.push(await push(tok.text, repo, "refs/heads/f", null, 90_000));
writeFileSync(`${OUT}/seed.json`, JSON.stringify({ login: LOGIN, repo: REPO, id: repo, owner: OWNER, packs }), { mode: 0o600 });
console.log(`seeded ${LOGIN}/${REPO} (repo ${repo}), 2 stored packs`);
