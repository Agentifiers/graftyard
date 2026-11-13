// Fresh m3org repositories for the am1-runs rows, by `POST /orgs/m3org/repos` as m3-admin (auto_init, so each has
// `main`), so no run shares a log. Writes `name id` lines to $M3_STATE/am1-runs-repos.txt (existing names kept).
//   node tests/am1/runs/mkrepos.mjs name...
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { EDGE, log, tokens } from "../hosted/lib.mjs";
const path = `${process.env.M3_STATE}/am1-runs-repos.txt`;
const have = Object.fromEntries(existsSync(path) ? readFileSync(path, "utf8").trim().split("\n").filter(Boolean).map((l) => l.split(" ")) : []);
for (const name of process.argv.slice(2)) {
  if (have[name]) continue;
  // Sent once, never re-sent: a re-sent create read "name already exists" while the first was still placing its
  // groups, and the GET that took it as done started a run on a repository with no `main` yet (fresh-repo, m23).
  const r = await fetch(`${EDGE}/api/v3/orgs/m3org/repos`, {
    method: "POST",
    headers: { authorization: `token ${tokens.admin}`, "content-type": "application/json", "user-agent": "graftyard-am1", accept: "application/vnd.github+json" },
    body: JSON.stringify({ name, private: false, auto_init: true }),
    signal: AbortSignal.timeout(400_000),
  }).then(async (r) => ({ status: r.status, body: await r.json().catch(() => null) }));
  if (r.status !== 201) throw new Error(`${name}: ${r.status} ${JSON.stringify(r.body?.message)}`);
  have[name] = r.body.id;
  log(`m3org/${name} = ${r.body.id}`);
}
writeFileSync(path, Object.entries(have).map(([n, i]) => `${n} ${i}`).join("\n") + "\n");
