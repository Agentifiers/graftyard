// The cold-write cell's repository on graft-m1 (`cold-write`'s hosted cell, tests/am1/results/b034-cw1.md):
// m3org/<name> made as tests/am1/b030/mkrepo.mjs makes repositories (`CreateRepo` on the org's `Authz`, then
// `RepoOwner`), then `main` with one commit pushed by m3-admin, and a `Read` grant to the B0-34 hostile owner
// (7180001, not an org member), so one caller may fetch and may not push. Idempotent. Prints `{"repo", "id", "main"}`.
//   node tests/coldwrite/hosted/setup.mjs [name]   (default b034-cw1)
import { writeFileSync } from "node:fs";
import { Git, OP, bed, commit, log, must, repoShard, state, tokens } from "../../am1/hosted/lib.mjs";
const name = process.argv[2] ?? "b034-cw1";
let id = (await bed({ chain: `authz:${state.org}`, read: { repo: name } })).value?.repo?.repo;
if (!id) {
  const o = await commit(`authz:${state.org}`, { cred: OP, op: { CreateRepo: { name } } });
  id = o?.ok?.out?.Repo?.repo;
  if (!id) throw new Error(`${name}: ${JSON.stringify(o).slice(0, 200)}`);
}
await must(`names:${repoShard(id)}`, { cred: OP, op: { RepoOwner: { repo: id, owner: state.org } } }, "Exists", "AlreadyOwned");
await must(`authz:${state.org}`, { cred: OP, op: { SetGrant: { repo: id, user: 7_180_001, permission: "Read" } } });
const g = new Git(`m3org/${name}`).as(tokens.admin);
if (!g.remoteHead("refs/heads/main")) {
  g.run(["init", "-q", "-b", "main", g.wt], g.dir);
  writeFileSync(`${g.wt}/README.md`, "cold-write cell\n");
  g.run(["add", "README.md"]);
  g.run(["commit", "-q", "-m", "cold-write cell"]);
  const p = g.run(["push", "-q", g.url, "main"]);
  if (!p.ok) throw new Error(`push main: ${p.out.slice(0, 300)}`);
}
const main = g.remoteHead("refs/heads/main");
log(`m3org/${name} = repo:${id}, main ${main}`);
console.log(JSON.stringify({ repo: `m3org/${name}`, id, main }));
