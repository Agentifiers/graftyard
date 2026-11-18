// Seeds one of B0-34's networks from a local bare clone through the edge's git-receive-pack, one push per line of a
// plan from tests/fanout/b034plan.py, with real `git push` under the named account's PAT (Human class). Resumable:
// the index of the last pushed line is kept in <plan>.<owner_repo>.done, so a rerun after a failure starts at the next line.
// The repository is kept between rounds (the operator's call: later B0-34 reruns skip the seed).
//   node tests/fanout/b034seed.mjs <bare dir> <plan file> <owner/repo> <admin|member>
// Environment as tests/am1/hosted/lib.mjs. The token goes only to git's credential helper, never on argv.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { Git, log, sleep, tokens } from "../am1/hosted/lib.mjs";

const [bare, planFile, repo, who] = process.argv.slice(2);
if (!bare || !planFile || !repo || !tokens[who]) throw new Error("usage: b034seed.mjs <bare> <plan> <owner/repo> <admin|member>");
const lines = readFileSync(planFile, "utf8").trim().split("\n").filter((l) => !l.startsWith("#")).map((l) => l.split(" "));
const doneFile = `${planFile}.${repo.replace(/\//g, "_")}.done`;
let from = existsSync(doneFile) ? Number(readFileSync(doneFile, "utf8")) + 1 : 0;
const g = new Git(repo).as(tokens[who]);
let bytes = 0;
const t0 = Date.now();
for (let i = from; i < lines.length; i++) {
  const [commit, ref, est] = lines[i];
  let r;
  // 12 tries over about 17 minutes: the orb's proxy dropped workers.dev for 3 to 4 minutes at a time on 2026-11-18
  // (18:06, 18:12, 18:46Z), past six tries' 3.75, while the edge itself served.
  for (let a = 0; a < 12; a++) {
    const s = Date.now();
    // `+`: the repository was created with `auto_init`, so its first `main` is not an ancestor of the seed.
    r = g.run(["-c", "http.postBuffer=104857600", "push", "-q", g.url, `+${commit}:${ref}`], bare);
    log(`${i + 1}/${lines.length} ${ref} est ${(Number(est) / 1e6).toFixed(1)} MB: ${r.ok ? "ok" : r.out.split("\n").filter(Boolean).slice(-2).join(" | ").slice(0, 200)} ${Date.now() - s} ms`);
    if (r.ok) break;
    await sleep(15_000 * Math.min(a + 1, 6));
  }
  if (!r.ok) throw new Error(`line ${i + 1} failed twelve times`);
  bytes += Number(est);
  writeFileSync(doneFile, String(i));
}
log(`seeded ${repo}: ${(bytes / 1e9).toFixed(2)} GB in ${Math.round((Date.now() - t0) / 1000)} s`);
