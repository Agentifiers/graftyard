// Recreates graft-m1's lease groups that no longer restore (j4, lf1-2, lf1-3: their logs hold lease events written
// at 33 to 36 under `9fb5032`, read at 35 to 38 since `6042f3e`). Each member is cleared with the test route `/retire`
// (storage deleted, object reset: BF-11's lost-member stub), then one `git ls-remote` through graft-m1-edge finds no
// group and creates it on first use (`ensure_group` → `graftyard_lookup::create`, joining the same generation-1 names
// with the repository's owner). The repository's Authz and Names rows are untouched; its chain starts empty.
//   node tests/m3/wirefix/recreate.mjs [name:id ...]   (default j4:1048576 lf1-2:1048579 lf1-3:1048580)
import { Git, log, member, statuses, tokens } from "../../am1/hosted/lib.mjs";

const groups = (process.argv.slice(2).length ? process.argv.slice(2) : ["j4:1048576", "lf1-2:1048579", "lf1-3:1048580"])
  .map((s) => s.split(":")).map(([name, id]) => ({ name, chain: `repo:${id}` }));
const brief = (s) => s.restoreError ? `restoreError ${s.restoreError}` : s.joined === false ? "not joined" : `${s.role} t${s.term} c${s.commit}`;

for (const g of groups) {
  log(`${g.name} before: ${(await statuses(g.chain, 4)).map((s) => `${s.site}:${brief(s)}`).join(" | ")}`);
  for (const site of [0, 1, 2, 3]) {
    // The object resets inside the call, so the answer is often no answer at all; `/status` after says what happened.
    // `reuse`: the create below joins the same names, which a plain retire now refuses (`retire-sticks`).
    const r = await member(g.chain, site, "/retire", { reuse: true }).catch((e) => ({ status: String(e).slice(0, 60) }));
    log(`${g.name} site ${site} /retire: ${r?.status} ${String(JSON.stringify(r?.v)).slice(0, 80)}`);
  }
  const git = new Git(`m3org/${g.name}`).as(tokens.admin);
  const t0 = Date.now();
  const ls = git.run(["ls-remote", git.url], git.dir);
  log(`${g.name} ls-remote: ok=${ls.ok} ${Date.now() - t0} ms ${JSON.stringify(ls.out.slice(0, 160))}`);
  log(`${g.name} after: ${(await statuses(g.chain, 4)).map((s) => `${s.site}:${brief(s)}`).join(" | ")}`);
  const c = git.clone();
  log(`${g.name} clone: ok=${c.ok} ${JSON.stringify(c.out.slice(0, 160))}`);
}
