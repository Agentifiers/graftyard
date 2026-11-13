// Fresh m3org repositories for the am1-runs rows, by `POST /orgs/m3org/repos` as m3-admin (auto_init, so each has
// `main`), so no run shares a log. Writes `name id` lines to $M3_STATE/am1-runs-repos.txt (existing names kept).
//   node tests/am1/runs/mkrepos.mjs name...
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { api, log, tokens } from "../hosted/lib.mjs";
const path = `${process.env.M3_STATE}/am1-runs-repos.txt`;
const have = Object.fromEntries(existsSync(path) ? readFileSync(path, "utf8").trim().split("\n").filter(Boolean).map((l) => l.split(" ")) : []);
for (const name of process.argv.slice(2)) {
  if (have[name]) continue;
  let r = await api("POST", "/api/v3/orgs/m3org/repos", tokens.admin, { name, private: false, auto_init: true });
  // `api` re-sends a call whose answer was lost; a create that landed then reads "name already exists" (m17s250).
  if (r.status === 422 && r.body?.errors?.[0]?.message === "name already exists on this account") {
    r = await api("GET", `/api/v3/repos/m3org/${name}`, tokens.admin);
    if (r.status === 200) r.status = 201;
  }
  if (r.status !== 201) throw new Error(`${name}: ${r.status} ${JSON.stringify(r.body?.message)}`);
  have[name] = r.body.id;
  log(`m3org/${name} = ${r.body.id}`);
}
writeFileSync(path, Object.entries(have).map(([n, i]) => `${n} ${i}`).join("\n") + "\n");
