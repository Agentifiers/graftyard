// Nine more repositories on m3org (lf1-0 .. lf1-8), created as the seed creates j4: `CreateRepo` on the org's Authz
// chain, then `RepoOwner` on the repository's name shard. Prints `name id` lines into ~/.graft-m3h/am1-repos.txt.
//   node tests/am1/hosted/repos.mjs
import { writeFileSync } from "node:fs";
import { OP, bed, commit, log, must, repoShard, state } from "./lib.mjs";
const lines = [];
for (let i = 0; i < 9; i++) {
  const name = `lf1-${i}`;
  const o = await commit(`authz:${state.org}`, { cred: OP, op: { CreateRepo: { name } } });
  let repo = o?.ok?.out?.Repo?.repo;
  if (!repo) repo = (await bed({ chain: `authz:${state.org}`, read: { repo: name } })).value?.repo?.repo;
  if (!repo) throw new Error(`${name}: ${JSON.stringify(o).slice(0, 200)}`);
  await must(`names:${repoShard(repo)}`, { cred: OP, op: { RepoOwner: { repo, owner: state.org } } });
  lines.push(`${name} ${repo}`);
  log(`m3org/${name} = ${repo}`);
}
writeFileSync(`${process.env.HOME}/.graft-m3h/am1-repos.txt`, lines.join("\n") + "\n");
