// B0-34's networks on graft-m2b, made once and kept between rounds (the operator's call): the monorepo `m3org/b034-mono`
// (seeded from torvalds/linux by b034seed.mjs), a second owner's network `m3-member/b034-second`, and a hostile owner
// `m3r-b034-hostile` (a fresh user, account 7180001, with its own PAT in $M3_STATE/b034-hostile.token, mode 0600)
// with three networks `b034-hostile-1..3`, which tests/fanout/b034.sh seeds from git/git (0.3 GB each, as M0's
// `gitnet` and `hostile1..3`). Writes $M3_STATE/b034.json: {mono, second, hostile}, each {repo, chain, who}, `who`
// naming the account's key in `tokens` (this adds `hostile` to $M3_STATE/m3-tokens.txt).
//   node tests/fanout/b034setup.mjs
// Environment as tests/m3/rows/lib.mjs. No token reaches argv or a log.
import { existsSync, readFileSync, writeFileSync, appendFileSync } from "node:fs";
import { user, pat } from "../m3/rows/lib.mjs";
import { OP, api, bed, commit, log, must, repoShard, state, tokens } from "../am1/hosted/lib.mjs";

const S = process.env.M3_STATE;
const out = `${S}/b034.json`;
const have = existsSync(out) ? JSON.parse(readFileSync(out, "utf8")) : {};
const HOSTILE = 7_180_001, HLOGIN = "m3r-b034-hostile";
const tokFile = `${S}/b034-hostile.token`;
if (!existsSync(tokFile)) {
  await user(HOSTILE, HLOGIN);
  writeFileSync(tokFile, (await pat(HOSTILE, "b034 hostile")).text, { mode: 0o600, flag: "wx" });
  log(`hostile owner ${HLOGIN} made`);
}
// a1.mjs reads `tokens[who]` from m3-tokens.txt; the hostile PAT is added as `hostile` once.
const toks = readFileSync(`${S}/m3-tokens.txt`, "utf8");
if (!toks.split("\n").some((l) => l.startsWith("hostile "))) appendFileSync(`${S}/m3-tokens.txt`, `hostile ${readFileSync(tokFile, "utf8").trim()}\n`);
const hostileTok = readFileSync(tokFile, "utf8").trim();

// B034_CREATE=owner (graft-m1, whose edge does not enable 32.13 for REST create): `CreateRepo` on the owner's Authz
// chain, then `RepoOwner` on the name shard, as tests/am1/b030/mkrepo.mjs makes m3org/b0-30. The repository starts
// with no `main` and no settings row; b034seed.mjs force-pushes the first line, so nothing needs auto_init.
const OWNERS = { m3org: state.org, "m3-member": state.member, [HLOGIN]: HOSTILE };
async function ownerRepo(owner, name) {
  const a = OWNERS[owner];
  const o = await commit(`authz:${a}`, { cred: OP, op: { CreateRepo: { name } } });
  const id = o?.ok?.out?.Repo?.repo ?? (await bed({ chain: `authz:${a}`, read: { repo: name } })).value?.repo?.repo;
  if (!id) throw new Error(`${owner}/${name}: ${JSON.stringify(o).slice(0, 200)}`);
  await must(`names:${repoShard(id)}`, { cred: OP, op: { RepoOwner: { repo: id, owner: a } } });
  return { repo: `${owner}/${name}`, chain: `repo:${id}` };
}
async function repo(owner, name, tok) {
  if (process.env.B034_CREATE === "owner") return ownerRepo(owner, name);
  const path = owner === "m3org" ? "/api/v3/orgs/m3org/repos" : "/api/v3/user/repos";
  let r = await api("POST", path, tok, { name, private: false, auto_init: true });
  if (r.status === 422) r = await api("GET", `/api/v3/repos/${owner}/${name}`, tok);
  if (r.status !== 201 && r.status !== 200) throw new Error(`${owner}/${name}: ${r.status} ${JSON.stringify(r.body?.message)}`);
  return { repo: `${owner}/${name}`, chain: `repo:${r.body.id}` };
}
have.mono ??= { ...(await repo("m3org", "b034-mono", tokens.admin)), who: "admin" };
have.second ??= { ...(await repo("m3-member", "b034-second", tokens.member)), who: "member" };
have.hostile ??= [];
for (let i = have.hostile.length; i < 3; i++) {
  have.hostile.push({ ...(await repo(HLOGIN, `b034-hostile-${i + 1}`, hostileTok)), who: "hostile" });
  writeFileSync(out, JSON.stringify(have, null, 2));
}
writeFileSync(out, JSON.stringify(have, null, 2));
log(`b034 networks: ${JSON.stringify(have)}`);
