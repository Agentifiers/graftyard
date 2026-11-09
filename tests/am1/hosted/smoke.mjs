// Hosted smoke of AM1's wiring on m3org/j4 with the m3-ci App flagged ThirdParty, confine `refs/heads/claude/**`:
// an external task, a claimed `ghs_` (`graftyard_task`), a push under the prefix with a `Graftyard-Session` trailer
// (linked), a forged trailer (claimed), a push outside the prefix and one touching a protected path (refused), and a
// REST ref write. Prints one JSON line per step to stdout and tests/am1/raw/smoke.jsonl.
//   node tests/am1/hosted/smoke.mjs
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { api, ghs, Git, HOST, log, logFrom, row, state } from "./lib.mjs";

const OUT = new URL("../raw/smoke.jsonl", import.meta.url).pathname;
mkdirSync(new URL("../raw/", import.meta.url).pathname, { recursive: true });
const R = "m3org/j4";
const step = (name, v) => { row(OUT, { step: name, ...v }); log(`${name} ${JSON.stringify(v).slice(0, 300)}`); };
const name = `smoke-${Date.now().toString(36)}`;

const t0 = await ghs({});
step("ghs", { status: t0.status });
const create = await api("POST", `/api/graftyard/v1/repos/${R}/tasks`, t0.body.token, { kind: "external", session: name, summary: "am1 smoke" });
step("create task", { status: create.status, id: create.body?.id, state: create.body?.state });
const id = create.body?.id ?? `${state.app}.${name}`;
const claimed = await ghs({ repositories: ["j4"], graftyard_task: id });
step("claimed ghs", { status: claimed.status, msg: claimed.body?.message });
const tok = claimed.status === 201 ? claimed.body.token : t0.body.token;

const g = new Git(R).as(tok);
step("clone", { ok: g.clone().ok });
const main = g.run(["rev-parse", "HEAD"]).out.trim();
const origin = `https://${HOST}/${R}`;
function commitPush(ref, file, msg) {
  g.run(["checkout", "-q", "--detach", main]);
  const p = join(g.wt, file);
  mkdirSync(join(p, ".."), { recursive: true });
  writeFileSync(p, `${Date.now()} ${Math.random()}\n`);
  g.run(["add", "-A"]);
  g.run(["commit", "-q", "-m", msg]);
  const sha = g.run(["rev-parse", "HEAD"]).out.trim();
  const r = g.run(["push", "-q", "origin", `HEAD:${ref}`]);
  return { ok: r.ok, sha, err: r.ok ? undefined : r.out.split("\n").filter((l) => /remote:|!|error/.test(l)).slice(0, 3).join(" | ") };
}
const before = (await logFrom(`repo:${state.repo}`, 1, 1)).length;
const linked = commitPush(`refs/heads/claude/${name}-a`, `am1/${name}.txt`, `smoke linked\n\nGraftyard-Session: ${origin}/_graftyard/sessions/${id}\n`);
step("push linked trailer", linked);
const forged = commitPush(`refs/heads/claude/${name}-b`, `am1/${name}-b.txt`, `smoke forged\n\nGraftyard-Session: ${origin}/_graftyard/sessions/${state.app}.not-a-task\n`);
step("push forged trailer", forged);
const outside = commitPush(`refs/heads/${name}-outside`, `am1/${name}-c.txt`, "smoke outside prefix");
step("push outside prefix (refused)", outside);
const prot = commitPush(`refs/heads/claude/${name}-d`, `.github/workflows/${name}.yml`, "smoke protected path");
step("push protected path (refused)", prot);
// REST ref write: a new ref under the prefix at main, then a non-fast-forward PATCH without force.
const w1 = await api("POST", `/api/v3/repos/${R}/git/refs`, tok, { ref: `refs/heads/claude/${name}-rest`, sha: main });
step("REST POST git/refs", { status: w1.status, ref: w1.body?.ref, msg: w1.body?.message });
const w2 = await api("PATCH", `/api/v3/repos/${R}/git/refs/heads/claude/${name}-a`, tok, { sha: main });
step("REST PATCH non-ff (422)", { status: w2.status, msg: w2.body?.message });
const w3 = await api("POST", `/api/v3/repos/${R}/git/refs`, tok, { ref: `refs/heads/${name}-rest-outside`, sha: main });
step("REST POST outside prefix (refused)", { status: w3.status, msg: w3.body?.message });
// What the leader stored: provenance sources per pushed commit.
const entries = await logFrom(`repo:${state.repo}`, Math.max(1, before), 500);
const mine = entries.filter((e) => (e.refs ?? []).some((r) => String(r[0]).includes(name)));
step("stored", { entries: mine.map((e) => ({ index: e.index, kinds: e.kinds, provenance: e.provenance })) });
const task = await api("GET", `/api/graftyard/v1/repos/${R}/tasks/${id}`, t0.body.token);
step("task", { status: task.status, state: task.body?.state, leases: task.body?.leases });
