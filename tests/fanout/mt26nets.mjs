// mt26's networks with mt24 and mt25 split: mono and hostile warm on b, second warm on c (warm site = the higher rendezvous
// weight of sites 1 and 2 for "network:<id>", graftyard_git::warm_sites). Each wanted name is created as
// b034setup.mjs's ownerRepo does; an id that pairs with the wrong site is kept unseeded as a burn and the next try
// uses the name with a letter added. Writes $M3_STATE/b034.json, which b034setup.mjs then keeps. Logs ids only.
// Run from the mt26 worktree root with the cloud env sourced.
import { createHash } from "node:crypto";
import { writeFileSync, existsSync } from "node:fs";
import { OP, bed, commit, log, must, repoShard, state } from "../am1/hosted/lib.mjs";
const S = process.env.M3_STATE, out = `${S}/b034.json`;
if (existsSync(out)) throw new Error(`${out} exists: archive it first`);
const warm = (n) => {
  const w = (s) => createHash("sha256").update(`network:${n}#`).update(Buffer.from([0, s])).digest().readBigUInt64BE(0);
  return w(1) > w(2) ? "b" : "c";
};
for (const [n, s] of [[1048621, "b"], [3473430, "c"], [1048619, "c"]]) if (warm(n) !== s) throw new Error(`warm(${n})`);
const HOSTILE = 7_180_001, HLOGIN = "m3r-b034-hostile";
const OWNERS = { m3org: state.org, "m3-member": state.member, [HLOGIN]: HOSTILE };
async function make(owner, name) {
  const a = OWNERS[owner];
  const o = await commit(`authz:${a}`, { cred: OP, op: { CreateRepo: { name } } });
  const id = o?.ok?.out?.Repo?.repo ?? (await bed({ chain: `authz:${a}`, read: { repo: name } })).value?.repo?.repo;
  if (!id) throw new Error(`${owner}/${name}: ${JSON.stringify(o).slice(0, 200)}`);
  await must(`names:${repoShard(id)}`, { cred: OP, op: { RepoOwner: { repo: id, owner: a } } });
  return id;
}
const burns = [];
async function want(owner, name, site) {
  for (const tail of ["", "b", "c", "d", "e", "f"]) {
    const id = await make(owner, name + tail);
    if (warm(id) === site) { log(`${owner}/${name + tail} repo:${id} warm on ${site}`); return { repo: `${owner}/${name + tail}`, chain: `repo:${id}` }; }
    burns.push(`${owner}/${name + tail} repo:${id}`);
    log(`${owner}/${name + tail} repo:${id} warm on ${warm(id)}: a burn, unseeded`);
  }
  throw new Error(`no id on ${site} for ${owner}/${name} in 6 tries`);
}
const have = {};
have.mono = { ...(await want("m3org", "b034-mono-q1", "b")), who: "admin" };
have.second = { ...(await want("m3-member", "b034-second-q1", "c")), who: "member" };
have.hostile = [];
for (let i = 1; i <= 3; i++) have.hostile.push({ ...(await want(HLOGIN, `b034-hostile-q1-${i}`, "b")), who: "hostile" });
writeFileSync(out, JSON.stringify(have, null, 2));
log(`b034 networks: ${JSON.stringify(have)}; burns: ${JSON.stringify(burns)}`);
