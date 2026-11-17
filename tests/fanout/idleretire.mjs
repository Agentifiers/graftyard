// Idle DO rows teardown (operator, 2026-11-16 after the hosted stop): clears every member of the finished lm1 test
// networks with the test route `/retire` (storage deleted, object reset), as tests/am1/b030split/retire.mjs did.
// Input: a JSON list of [site, do_name] (from durableObjectsPeriodicGroups, the members that wrote rows), argv[2].
// Each member's `/status` before and after; "not joined" after is done. Then purge.py deletes the networks' bucket objects.
import { appendFileSync, readFileSync } from "node:fs";
import { member } from "../am1/hosted/lib.mjs";

const LOG = process.env.RETIRE_LOG ?? "tests/fanout/raw/idle-retire-m2b.log";
const log = (s) => { const l = `${new Date().toISOString()} ${s}`; console.log(l); appendFileSync(LOG, l + "\n"); };
const MEMBERS = JSON.parse(readFileSync(process.argv[2], "utf8")).map(([s, n]) => [n.split("#")[0], s, n]);
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
const brief = (v) => v?.joined === false ? "not joined" : v?.error ? `error ${v.error}` : `${v?.role} t${v?.term} c${v?.commit}`;
const each = async (f) => { const q = [...MEMBERS]; await Promise.all(Array.from({ length: 8 }, async () => { while (q.length) await f(...q.shift()); })); };
await each(async (c, s, n) => {
  const before = brief(await status(c, s, n));
  const r = await member(c, s, "/retire", {}, 60_000, n).catch((e) => ({ status: String(e).slice(0, 80) }));
  log(`${n} site ${s} before: ${before}; /retire: ${JSON.stringify(r?.v ?? r?.status ?? null).slice(0, 60)}`);
});
await new Promise((r) => setTimeout(r, 15_000));
let done = 0;
await each(async (c, s, n) => { const b = brief(await status(c, s, n)); if (b === "not joined") done++; else log(`${n} site ${s} after: ${b}`); });
log(`retired ${done}/${MEMBERS.length} not joined`);
// R25 (operator, 2026-11-17): a retired test group's packs are test-harness state, so the harness deletes them. Once
// every member of this run's list is "not joined", purge.py deletes each of its repository networks' objects from every
// site bucket, after checking the group's members itself; NO_PURGE=1 skips it. The sweeper never does this on a guess.
const NETS = [...new Set(MEMBERS.map(([c]) => c.match(/^repo:(\d+)$/)?.[1]).filter(Boolean))];
if (done === MEMBERS.length && NETS.length && !process.env.NO_PURGE) {
  const { execFileSync } = await import("node:child_process");
  const out = execFileSync("uv", ["run", "-q", "--with", "boto3", "python3", "tests/fanout/purge.py", ...NETS], { encoding: "utf8" });
  for (const l of out.trim().split("\n")) log(`purge: ${l.slice(21)}`);
} else if (NETS.length) log(`purge skipped: ${done === MEMBERS.length ? "NO_PURGE" : "not every member retired"}`);
