// B0-30 rerun watch readings, kept in the repo since an orb restart clears /tmp (2026-11-16 04:11Z). Read-only.
//   node tests/am1/b030split/read.mjs leader   one line: the parent leader's archive margin and slim cursor
//   node tests/am1/b030split/read.mjs child    each member of the child and the parent: role, applied, SQLite, split
//   node tests/am1/b030split/read.mjs stats    each parent member's read and install counters (member /stats)
import { member, statuses, voter } from "../hosted/lib.mjs";

const C = "repo:1048590", CHILD = "repo-meta:1048590";
const names = (sts) => {
  const vs = sts.find((s) => s.config?.config?.normal?.voters)?.config.config.normal.voters;
  return vs ? Object.fromEntries(vs.map((v) => [v.site, v.do_name])) : {};
};
const out = (r) => console.log(JSON.stringify(r));

const what = process.argv[2] ?? "leader";
if (what === "leader") {
  const sts = await statuses(C);
  const led = sts.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  if (!led) throw new Error("no parent leader");
  const v = (await voter(C, led.site, "/bench_seed", { b030: { size: true } }, 60_000)).v;
  out({ at: new Date().toISOString(), site: led.site, term: led.term, applied: v.applied, archivedTo: v.archivedTo,
    truncatedTo: v.truncatedTo, promotedTo: v.promotedTo, logBytes: v.logBytes, unpromoted: v.unpromotedBytes, db: v.dbBytes,
    full: v.sqliteFull, slim: v.split?.slim ?? null, dropped: v.split?.state?.dropped ?? null });
} else if (what === "child") {
  for (const chain of [CHILD, C]) {
    const sts = await statuses(chain);
    const n = names(sts);
    for (const s of [0, 1, 2]) {
      const v = (await member(chain, s, "/bench_seed", { b030: { size: true } }, 60_000, n[s]).catch((e) => ({ v: { error: String(e).slice(0, 120) } }))).v;
      const st = sts.find((x) => x.site === s) ?? {};
      out({ at: new Date().toISOString(), chain, site: s, name: n[s], role: st.role, term: st.term, leader: st.leader,
        applied: v.applied, commit: v.commit, dbBytes: v.dbBytes, logBytes: v.logBytes, sqliteFull: v.sqliteFull,
        childLoaded: v.split?.childLoaded, seed: v.split?.state?.seed?.[0], dropped: v.split?.state?.dropped,
        slim: v.split?.slim ?? null, error: v.error });
    }
  }
} else if (what === "stats") {
  const n = names(await statuses(C));
  for (const s of [0, 1, 2]) {
    const v = (await member(C, s, "/stats", undefined, 20_000, n[s]).catch((e) => ({ v: { error: String(e).slice(0, 80) } }))).v;
    out({ at: new Date().toISOString(), site: s, leader: v.role, installs: v.installs, restores: v.restores,
      truncatedTo: v.truncatedTo, shedReads: v.shedReads, followerReads: v.followerReads, followerLate: v.followerLate,
      readStages: v.readStages, error: v.error });
  }
} else {
  throw new Error(`unknown reading ${what}`);
}
