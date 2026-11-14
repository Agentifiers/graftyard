// stream-push's hosted pushes: one new branch per run on m3org/stream-push, a root commit of <MiB> of random files,
// pushed by git through the edge as m3-admin, then read back with ls-remote. Prints one JSON line.
//   node tests/streampush/push.mjs <branch> <MiB>
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { Git, log, tokens } from "../am1/hosted/lib.mjs";

const [branch, mib] = [process.argv[2], Number(process.argv[3])];
const g = new Git("m3org/stream-push").as(tokens.admin);
execFileSync("git", ["init", "-q", "-b", branch, g.wt]);
let left = mib;
for (let i = 0; left > 0 || i === 0; i++) {
  const n = Math.min(left, 64);
  const f = join(g.wt, `f${i}`);
  writeFileSync(f, "");
  for (let k = 0; k < n; k++) writeFileSync(f, randomBytes(1 << 20), { flag: "a" });
  if (n === 0) writeFileSync(f, "small\n");
  left -= n;
}
g.run(["add", "."]);
g.run(["commit", "-qm", `stream-push ${branch}`]);
const oid = g.run(["rev-parse", "HEAD"]).out.trim();
g.run(["remote", "add", "origin", g.url]);
const t0 = Date.now();
const r = g.run(["-c", "http.postBuffer=1048576", "push", "origin", `${branch}:refs/heads/${branch}`]);
const ms = Date.now() - t0;
const head = g.remoteHead(`refs/heads/${branch}`);
log(JSON.stringify({ branch, mib, ok: r.ok, ms, landed: head === oid, out: r.out.split("\n").filter(Boolean).slice(-4) }));
execFileSync("rm", ["-rf", g.dir]);
