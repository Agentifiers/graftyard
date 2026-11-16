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
import { api, log, tokens } from "../am1/hosted/lib.mjs";

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

async function repo(owner, name, tok) {
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
