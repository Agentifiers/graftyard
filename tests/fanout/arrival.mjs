// arrival.mjs CHAIN MODE: where every voter of CHAIN keeps BF-14's `/commit` arrival rows, through test-faults
// `/stats?arrival=mem|sql` (`Host::arrivals_mem`). `sql` is a bare `exec` per `/commit`, its own implicit transaction
// outside the entry's (`tests/m0/results/carried-6.md`); `mem` writes nothing. Exits 1 unless every voter answers
// with MODE in `arrival`. Read-only otherwise; no token reaches argv or a log.
//   node tests/fanout/arrival.mjs repo:ID mem
import { log, member, voterNames } from "../am1/hosted/lib.mjs";

const [chain, mode] = process.argv.slice(2);
if (!/^(repo|authz):/.test(chain ?? "") || !["mem", "sql"].includes(mode)) throw new Error("usage: arrival.mjs repo:ID mem|sql");
const names = await voterNames(chain).catch(() => ({}));
let ok = true;
for (const s of [0, 1, 2]) {
  const r = await member(chain, s, `/stats?summary=1&arrival=${mode}`, undefined, 15_000, names[s]).catch((e) => ({ status: 0, v: { error: String(e) } }));
  const got = r.v?.arrival ?? null;
  log(`${chain} site ${s}: arrival ${got} (asked ${mode}, status ${r.status})`);
  if (got !== mode) ok = false;
}
process.exit(ok ? 0 : 1);
