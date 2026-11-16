// BF-54's job sampler: every job's state on `m3org/wp4-bf54` every 20 s, to `<out>/jobs.jsonl`, beside the driver.
// The driver counts a job started once `Assigned`; a match whose answer never reached its runner sits `Assigned` until
// its lease, so a job's real start is its first `InProgress` (the runner's `acquirejob`) or its `Completed`.
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp4/bf54-jobs.mjs tests/m4/raw/wp4/bf54
import { appendFileSync } from "node:fs";
import { join } from "node:path";
import { api, sleep, tokens } from "../../am1/hosted/lib.mjs";

// `MIN_RUN` skips older runs' ids, `PER_PAGE` lists fewer runs, `EVERY_MS` sets the pause, and `FILE` names the output
// file. Run 2 used them for a second sampler of its own runs; it still sampled only every 60 to 90 s, as one run's read
// took up to a minute while the shard served the run. Use the REST jobs list's `started_at` for start times.
const out = process.argv[2] ?? "tests/m4/raw/wp4/bf54";
const A = "/api/graftyard/v1/repos/m3org/wp4-bf54/actions";
const MIN_RUN = Number(process.env.MIN_RUN ?? 0), PER_PAGE = Number(process.env.PER_PAGE ?? 50);
const EVERY = Number(process.env.EVERY_MS ?? 20_000), FILE = process.env.FILE ?? "jobs.jsonl";
for (;;) {
  const r = await api("GET", `${A}/runs?per_page=${PER_PAGE}`, tokens.admin);
  const t = Date.now(), jobs = [];
  for (const run of (r.body?.runs ?? []).filter((x) => x.id >= MIN_RUN)) {
    const d = await api("GET", `${A}/runs/${run.id}`, tokens.admin);
    for (const j of d.body?.value?.jobs ?? []) {
      const [k, v] = Object.entries(j.state)[0] ?? ["?", {}];
      jobs.push([j.spec.id, run.id, k, v.runner ?? null, v.lease_until ?? v.at ?? v.since ?? null, j.version]);
    }
  }
  appendFileSync(join(out, FILE), JSON.stringify({ t, jobs }) + "\n");
  await sleep(EVERY);
}
