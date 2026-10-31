// B0-34 third pass (build/BUILD-PLAN.md, M0-WP9): prepares and uploads the networks for `run/wp9-git.ts` into the
// bucket `graft-wp9-git`, resumably. Each finished step leaves a marker in /home/user/wp9-data/up/, so a rerun skips it.
//
//   node run/wp9-gitprep.ts [--chain 220]
//
// For `linux` (a fresh bare clone of torvalds/linux, one 6.47 GB pack, plus a commit-graph; files over the upload limit
// go up as 250 MB byte parts `<name>.partNNN`, which the shard joins in `setup`):
// - chain: the first-parent commits from master~N to master, oldest first. The network's tip starts at chain[0] and
//   moves one step per queue merge (10 a minute) during the run, so composed misses meet a moving tip and the base
//   renews every 20 moves.
// - refs: chain[0]. base.pack and base.objs: chain[0]'s depth-1 pack, built as upload-pack builds it, and its objects.
// - commits: 400 real non-merge commits from the 30 days before chain[0] (the agent PRs' changes), merges: the parents
//   of the 60 first-parent merges before chain[0] (the queue's merges).
// For `git` (git/git): the same with a chain of one commit.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { chain: { type: "string", default: "220" } } });
const DATA = "/home/user/wp9-data", UP = `${DATA}/up`, ROOT = new URL("../", import.meta.url).pathname;
mkdirSync(UP, { recursive: true });
const sh = (cmd: string) => execFileSync("bash", ["-c", cmd], { encoding: "utf8", maxBuffer: 1 << 30 }).trim();
const log = (...a: unknown[]) => console.log(new Date().toISOString().slice(11, 19), ...a);
const done = (m: string) => existsSync(`${UP}/${m}`);
const mark = (m: string) => writeFileSync(`${UP}/${m}`, new Date().toISOString());

function prep(net: "linux" | "git", chainLen: number): void {
  const repo = `${DATA}/${net}.git`, out = `${DATA}/${net}.in`;
  mkdirSync(out, { recursive: true });
  const g = (a: string) => sh(`git -C ${repo} ${a}`);
  if (!done(`${net}.graph`)) { g("commit-graph write --reachable"); mark(`${net}.graph`); }
  if (!done(`${net}.meta`)) {
    const chain = g(`rev-list --first-parent --reverse -n ${chainLen + 1} master`).split("\n");
    const start = chain[0], startAt = Number(g(`log -1 --format=%ct ${start}`));
    writeFileSync(`${out}/chain`, chain.join("\n") + "\n");
    writeFileSync(`${out}/refs`, `${start} refs/heads/master\n`);
    writeFileSync(`${out}/commits`, g(`rev-list --no-merges -n 400 --after=${startAt - 30 * 86_400} ${start}`) + "\n");
    writeFileSync(`${out}/merges`, g(`log --first-parent --merges -n 60 --format=%P ${start}`) + "\n");
    for (const f of ["commits", "merges"]) writeFileSync(`${DATA}/${net}.${f}`, sh(`cat ${out}/${f}`) + "\n");
    mark(`${net}.meta`);
  }
  if (!done(`${net}.base`)) {
    const start = sh(`head -1 ${out}/chain`);
    log(net, "base pack of", start);
    sh(`printf -- '--shallow %s\\n%s\\n' ${start} ${start} | git -C ${repo} pack-objects --revs --shallow --stdout --delta-base-offset > ${out}/base.pack`);
    sh(`git -C ${repo} rev-list --objects --no-walk ${start} | cut -c1-40 > ${out}/base.objs`);
    mark(`${net}.base`);
  }
  if (!done(`${net}.packs`)) {
    for (const f of readdirSync(`${repo}/objects/pack`)) if (/^pack-.*\.(pack|idx|rev)$/.test(f)) sh(`ln -f ${repo}/objects/pack/${f} ${out}/${f}`);
    if (existsSync(`${repo}/objects/info/commit-graph`)) sh(`ln -f ${repo}/objects/info/commit-graph ${out}/commit-graph`);
    mark(`${net}.packs`);
  }
  // `wrangler r2 object put` takes at most 300 MiB, so larger files are split into 250 MB byte parts.
  if (!done(`${net}.split`)) {
    for (const f of readdirSync(out)) if (!f.includes(".part") && statSync(`${out}/${f}`).size > 250e6) { sh(`split -b 250000000 -d -a 3 ${out}/${f} ${out}/${f}.part`); sh(`rm ${out}/${f}`); }
    mark(`${net}.split`);
  }
  // Upload every file, largest first; one marker per object.
  const files = readdirSync(out).map((f) => ({ f, size: statSync(`${out}/${f}`).size })).sort((a, b) => b.size - a.size);
  for (const { f, size } of files) {
    const m = `${net}.put.${f}`;
    if (done(m)) continue;
    log(net, "put", f, size);
    for (let i = 0; ; i++) {
      try { execFileSync("npx", ["wrangler", "r2", "object", "put", `graft-wp9-git/net/${net}/${f}`, "--file", `${out}/${f}`, "--remote"], { cwd: ROOT, stdio: ["ignore", "ignore", "pipe"] }); break; }
      catch (e) { if (i >= 3) throw e; log("retry", f, String(e).slice(0, 200)); }
    }
    mark(m);
  }
  log(net, "uploaded", files.length, "objects");
}

try { execFileSync("npx", ["wrangler", "r2", "bucket", "create", "graft-wp9-git", "--location", "enam"], { cwd: ROOT, stdio: "ignore" }); } catch { /* exists */ }
prep("git", 0);
prep("linux", Number(opt.chain));
log("all done");
