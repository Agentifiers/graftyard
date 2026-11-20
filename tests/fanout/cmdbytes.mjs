// Bytes of one repository's tier reads by shape, read on its leader through the tools' `/member` (test installs): the
// packed whole-tier dump every task, lease and token-claim read streamed before `command-rows`, against the command
// reads that replaced it (`decisions/DECISIONS.md`, "`command-rows`"). Read after a row, before teardown.
//   node tests/fanout/cmdbytes.mjs repo:ID [session]   # session: a task id `<app>.<name>` for the task read
// Environment as tests/am1/hosted/lib.mjs. No token reaches argv or a log.
import { leaderOf, member } from "../am1/hosted/lib.mjs";

const [chain, session] = process.argv.slice(2);
if (!chain?.startsWith("repo:")) throw new Error("usage: cmdbytes.mjs repo:ID [session]");
const leader = await leaderOf(chain);
const shapes = {
  packed: { rows: true, ranges: true, packed: true },
  lease: { rows: { access: true, command: { owner: 0 } }, ranges: true },
  task: { rows: { access: true, command: { session, loaded: true, owner: 0 } }, ranges: true },
};
const out = { chain, leader };
for (const [k, q] of Object.entries(shapes)) {
  if (k === "task" && !session) continue;
  const r = await member(chain, leader, "/read", { q }, 30_000);
  const v = r.v?.value;
  out[k] = { bytes: JSON.stringify(v ?? null).length, command: v?.command === true, index: r.v?.index };
}
console.log(JSON.stringify(out));
