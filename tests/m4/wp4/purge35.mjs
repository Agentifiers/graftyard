// Purges graft-m4a's leftover test repositories through the product route (`workers/edge/src/purge.rs`), as a site
// admin (m3-admin). Per repository id: DELETE /repos/{o}/{r} if it still answers, then
// POST /api/graftyard/v1/admin/repos/{id}/purge with {"early": true}. A 409 for a tombstone not yet applied, or a
// member that did not answer, is timing and is asked again; any other 409 stops the run. `second` sends the second
// pass for every id once its `secondPassAt` has passed. Every reply goes to tests/m4/raw/wp4/purge35.jsonl.
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp4/purge35.mjs first|second ID|FIRST-LAST ...
import { api, log, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const OUT = new URL("../raw/wp4/purge35.jsonl", import.meta.url).pathname;
const [pass, ...args] = process.argv.slice(2);
const ids = args.flatMap((a) => {
  const [x, y] = a.split("-").map(Number);
  return y ? Array.from({ length: y - x + 1 }, (_, i) => x + i) : [x];
});
if (!["first", "second"].includes(pass) || !ids.length) throw new Error("usage: purge35.mjs first|second ID|FIRST-LAST ...");
if (ids.some((id) => id === 0 || id === 65536)) throw new Error("repo:0 and repo:65536 are kept");
const admin = tokens.admin;
const TIMING = new Set(["A member has not applied the repository's tombstone", "A member did not answer"]);

for (const id of ids) {
  if (pass === "first") {
    const r = await api("GET", `/api/v3/repositories/${id}`, admin);
    if (r.status === 200) {
      const d = await api("DELETE", `/api/v3/repos/${r.body.full_name}`, admin);
      row(OUT, { at: new Date().toISOString(), id, step: "delete", repo: r.body.full_name, status: d.status });
      log(`${id} DELETE ${r.body.full_name} ${d.status}`);
      if (d.status !== 204) throw new Error(`${id}: DELETE answered ${d.status}`);
    } else if (r.status !== 404) throw new Error(`${id}: GET answered ${r.status}`);
  }
  for (let i = 0; ; i++) {
    const p = await api("POST", `/api/graftyard/v1/admin/repos/${id}/purge`, admin, { early: true });
    row(OUT, { at: new Date().toISOString(), id, step: pass, attempt: i, status: p.status, ms: p.ms, body: p.body });
    log(`${id} ${pass} #${i} ${p.status} ${JSON.stringify(p.body).slice(0, 240)}`);
    if (p.status === 200) break;
    const timing = p.status === 409 ? TIMING.has(p.body?.message) : p.status === 503 || p.status === 0;
    if (!timing) throw new Error(`${id}: ${pass} refused (${p.status}), not timing; stopping`);
    if (i >= 9) throw new Error(`${id}: ${pass} still refused after ${i + 1} tries`);
    await sleep(20_000);
  }
}
log(`${pass} pass done: ${ids.length} ids`);
