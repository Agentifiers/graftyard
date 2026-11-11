// idle-shards' active control on graft-m2b: a fresh m3org repository, a push to a branch, a pull request from it, and
// a head push. Each step's answer, then whether `refs/pull/N/head` reached the head and `mergeable` was computed,
// which needs the network's git cache shard. Prints JSON lines, no token.
//   node tests/idleshards/active.mjs NAME
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { Git, api, log, sleep, tokens } from "../am1/hosted/lib.mjs";

const name = process.argv[2];
if (!name) throw new Error("usage: active.mjs NAME");
const admin = tokens.admin;
const out = (step, v) => console.log(JSON.stringify({ at: new Date().toISOString(), step, ...v }));
const c = await api("POST", "/api/v3/orgs/m3org/repos", admin, { name, private: false, auto_init: true });
out("create", { status: c.status, id: c.body?.id });
const R = `m3org/${name}`;
const g = new Git(R).as(admin);
out("clone", { ok: g.clone().ok });
const commit = (f, s) => {
  mkdirSync(join(g.wt, "d"), { recursive: true });
  writeFileSync(join(g.wt, "d", f), `${s}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", `${f} ${s}`]);
};
g.run(["checkout", "-q", "-b", "topic"]);
commit("a.txt", "one");
let p = g.run(["push", "-q", "origin", "HEAD:refs/heads/topic"]);
out("push topic", { ok: p.ok, err: p.ok ? undefined : p.out.slice(0, 200) });
const pr = await api("POST", `/api/v3/repos/${R}/pulls`, admin, { title: "idle-shards control", head: "topic", base: "main" });
out("open pr", { status: pr.status, number: pr.body?.number });
const n = pr.body?.number;
commit("b.txt", "two");
p = g.run(["push", "-q", "origin", "HEAD:refs/heads/topic"]);
const head = g.run(["rev-parse", "HEAD"]).out.trim();
out("head push", { ok: p.ok, head });
for (let i = 0; i < 24 && n; i++) {
  await sleep(5000);
  const ref = g.remoteHead(`refs/pull/${n}/head`);
  const v = await api("GET", `/api/v3/repos/${R}/pulls/${n}`, admin);
  if (ref === head && v.body?.mergeable !== null && v.body?.mergeable !== undefined) {
    out("synced", { after_s: (i + 1) * 5, pull_head: ref === head, mergeable: v.body.mergeable });
    process.exit(0);
  }
}
out("not synced", { pull_head: g.remoteHead(`refs/pull/${n}/head`) });
log("done");
