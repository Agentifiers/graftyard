// m3org/b0-30 on graft-m1, made as tests/am1/hosted/repos.mjs makes repositories (`CreateRepo` on the org's Authz
// chain, then `RepoOwner` on the name shard; graft-m1's edge does not enable 32.13 for REST create), then `main`
// with one commit pushed by m3-admin. Idempotent. Prints `{"repo": id}` and writes it to tests/am1/raw/b030-repo.json.
//   node tests/am1/b030/mkrepo.mjs [name]   (default b0-30; a new name makes a new repository and repoints the file)
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { Git, OP, bed, commit, log, must, repoShard, state, tokens } from "../hosted/lib.mjs";
const out = "tests/am1/raw/b030-repo.json";
const name = process.argv[2] ?? "b0-30";
const was = existsSync(out) ? JSON.parse(readFileSync(out, "utf8")) : null;
let repo = was?.name === `m3org/${name}` ? was.repo : null;
if (!repo) {
  const o = await commit(`authz:${state.org}`, { cred: OP, op: { CreateRepo: { name } } });
  repo = o?.ok?.out?.Repo?.repo ?? (await bed({ chain: `authz:${state.org}`, read: { repo: name } })).value?.repo?.repo;
  if (!repo) throw new Error(`${name}: ${JSON.stringify(o).slice(0, 200)}`);
  await must(`names:${repoShard(repo)}`, { cred: OP, op: { RepoOwner: { repo, owner: state.org } } });
  writeFileSync(out, JSON.stringify({ name: `m3org/${name}`, repo }) + "\n");
}
const g = new Git(`m3org/${name}`).as(tokens.admin);
if (!g.remoteHead("refs/heads/main")) {
  g.run(["init", "-q", "-b", "main", g.wt], g.dir);
  writeFileSync(`${g.wt}/README.md`, "B0-30 hot-group storage\n");
  g.run(["add", "README.md"]);
  g.run(["commit", "-q", "-m", "B0-30"]);
  const p = g.run(["push", "-q", g.url, "main"]);
  if (!p.ok) throw new Error(`push main: ${p.out.slice(0, 300)}`);
}
log(`m3org/${name} = repo:${repo}, main ${g.remoteHead("refs/heads/main")}`);
