// B0-28 replay and the negative test for run/wp8.ts. The test owns the calendar: it sets every member's clock (`/clock`)
// to a simulated time, pushes, and runs one maintenance pass (`/tick`: archive, GC, sweep, snapshot, truncate) per step.
//
// b028: day -40 to day 0, a repository with main, feature, and release and daily pushes, so archives, snapshots, GC, and
// truncation are all running by day 0. Day 0 records the targets. Day 1 deletes `feature` and force-pushes `release` to
// an unrelated root, and the old release tip's objects become unreachable from refs. Days 2 to 30 keep pushing main,
// with a leader change on day 12 and follower C offline from day 16 to day 24, so it rejoins past the archive boundary
// by InstallSnapshot. Day 30 restores both refs to day 0 and checks objects from R2. Then the noreflog control.

import { parseArgs } from "node:util";
import { DAY, edge, iso, Repo, save, sleep } from "./wp8.ts";

const T0 = Date.parse("2026-11-01T12:00:00Z"); // simulated day 0
type Views = { status: { site: string; role: string; base: number; applied: number; archThrough: number; snapThrough: number; refusedTruncations: number; installs: number; notes: string[] } }[];

async function tick(group: string, log: unknown[], label: string) {
  const r = await edge("/tick", { group });
  log.push({ label, tick: r });
  return r;
}
async function setDay(group: string, day: number, hour = 0) { await edge("/clock", { group, cal: T0 + day * DAY + hour * 3_600_000 }); }

async function start(group: string): Promise<void> {
  await edge("/clock", { group, cal: T0 - 41 * DAY });
  for (let i = 0; i < 5; i++) {
    const r = await edge<{ result: string }>("/campaign", { group, site: "a" });
    if (r.result.startsWith("leader") || r.result.startsWith("already")) return;
    await sleep(1000);
  }
  throw new Error("no leader");
}

/** One push through the edge; `force` skips the old-value check by reading the current value first. */
async function push(group: string, repo: Repo, ref: string, old: string | null, tip: string | null) {
  const objects = tip ? repo.objects(tip) : [];
  const r = await edge("/push", { group, rid: crypto.randomUUID(), updates: [{ ref, old, new: tip }], objects });
  if (!r.out?.ok) throw new Error(`push ${ref} failed: ${JSON.stringify(r).slice(0, 300)}`);
  return r;
}

export async function main(cmd: string, groupArg?: string): Promise<void> {
  if (cmd === "neg") return negative(groupArg ?? `neg-${Date.now().toString(36)}`);
  if (cmd === "install") return install(groupArg ?? `inst-${Date.now().toString(36)}`);
  if (cmd === "window") return window(groupArg ?? `win-${Date.now().toString(36)}`);
  const group = groupArg ?? `b028-${Date.now().toString(36)}`;
  const variant = group.startsWith("noreflog-") ? "noreflog" : "design";
  const log: unknown[] = [];
  const t = Date.now();
  await start(group);
  const repo = new Repo();
  const tips: Record<string, string | null> = { main: null, feature: null, release: null };
  const mv = async (ref: string, tip: string | null, force = false) => {
    await push(group, repo, `refs/heads/${ref}`, tips[ref], tip);
    void force; tips[ref] = tip;
  };
  // Days -40 to 0: history. A big file every 5 days makes a stored pack; the rest are inline.
  for (let d = -40; d <= 0; d++) {
    await setDay(group, d, 1);
    tips.main = tips.main; await mv("main", repo.commit(tips.main, `main day ${d}`, [`src/m${d % 7}.txt`], d % 5 === 0 ? 12_000 : 200));
    if (d === -38) await mv("feature", repo.commit(tips.main, "feature start", ["feat/a.txt"], 300));
    if (d === -37) await mv("release", repo.commit(tips.main, "release start", ["rel/r.txt"], 300));
    if (d > -38 && d % 3 === 0) await mv("feature", repo.commit(tips.feature, `feature day ${d}`, ["feat/a.txt", `feat/b${d % 4}.txt`], d % 6 === 0 ? 10_000 : 400));
    if (d > -37 && d % 4 === 0) await mv("release", repo.commit(tips.release, `release day ${d}`, [`rel/r${d % 3}.txt`], 9_000));
    await setDay(group, d, 3);
    await tick(group, log, `day ${d}`);
    process.stdout.write(`day ${d} `);
  }
  const day0 = { feature: tips.feature, release: tips.release, main: tips.main };
  await setDay(group, 0, 4);
  const day0Read = await edge("/read", { group, refs: { feature: day0.feature!, release: day0.release! } });
  console.log(`\nday 0 targets ${JSON.stringify(day0)}; closure ${day0Read.reached} objects, missing ${day0Read.missing.length}`);
  // Day 1: delete feature, force-push release to an unrelated root commit.
  await setDay(group, 1, 2);
  await mv("feature", null);
  await mv("release", repo.commit(null, "release rewritten", ["other/x.txt"], 500), true);
  const day1 = { feature: tips.feature, release: tips.release };
  // Days 1 to 30 with GC and archiving running.
  for (let d = 1; d <= 30; d++) {
    await setDay(group, d, 5);
    await mv("main", repo.commit(tips.main, `main day ${d}`, [`src/m${d % 7}.txt`], d % 5 === 0 ? 12_000 : 200));
    if (d === 12) log.push({ label: "leader change to b", r: await edge("/campaign", { group, site: "b" }) });
    if (d === 16) log.push({ label: "c offline", r: await edge("/flags", { group, site: "c", flags: { offline: true } }) });
    if (d === 25) log.push({ label: "c online", r: await edge("/flags", { group, site: "c", flags: {} }) });
    await setDay(group, d, 6);
    await tick(group, log, `day ${d}`);
    if (d === 25) { await sleep(3000); await tick(group, log, `day ${d} rejoin`); }
    process.stdout.write(`day ${d} `);
  }
  console.log();
  // Day 30: restore both refs to the last index at or before day 0's end of pushes.
  await setDay(group, 30, 8);
  const before = await edge("/audit", { group });
  const idx = await edge("/indexAt", { group, t: T0 + 4 * 3_600_000 });
  const restore = await edge("/restore", { group, rid: `restore-${group}`, to: idx.to, refs: ["refs/heads/feature", "refs/heads/release"] });
  await sleep(2000);
  await tick(group, log, "day 30 after restore");
  const refs = await edge("/refs", { group });
  const afterRead = await edge("/read", { group });
  const audit = await edge("/audit", { group });
  const views = await edge<Views>("/view", { group });
  const got = (refs as { site: string; refs: Record<string, string> }[]).map((r) => ({ site: r.site, feature: r.refs?.["refs/heads/feature"], release: r.refs?.["refs/heads/release"] }));
  const pass = restore.out?.ok === true && got.every((g) => g.feature === day0.feature && g.release === day0.release)
    && afterRead.missing.length === 0 && afterRead.badSha1 === 0 && afterRead.unreadable.length === 0
    && audit.badTruncations.length === 0 && audit.committedKeysNotInTwoBuckets.length === 0;
  const result = { group, variant, wallSeconds: Math.round((Date.now() - t) / 1000), day0, day1, indexAt: idx, restore, refs: got,
    readAfter: afterRead, auditBefore: before, audit, pass,
    members: views.map((v) => ({ ...v.status, notes: v.status.notes.slice(-60) })) };
  save(`wp8-${group}.json`, result);
  save(`wp8-${group}-ticks.json`, log);
  console.log(JSON.stringify({ group, pass, restore: restore.out ?? restore, check: restore.check && { ...restore.check, missing: restore.check.missing.length },
    refs: got, read: { reached: afterRead.reached, missing: afterRead.missing.length, bad: afterRead.badSha1, byType: afterRead.byType },
    audit: { archives: audit.archives, snapshots: audit.snapshots, bad: audit.badTruncations, notInTwo: audit.committedKeysNotInTwoBuckets.length, truncations: audit.truncations, fold: audit.foldDigests } }, null, 1));
}

/** Follower offline past the archive boundary (design/DESIGN.md, Milestone 3 row): c is offline from day -36 to day 0
 *  while a pushes daily and archives, snapshots, GCs, and truncates; the leader's base passes c's log, so c must rejoin by
 *  `InstallSnapshot` from the newest committed snapshot plus archives, with a fold digest equal to the others'. */
async function install(group: string): Promise<void> {
  await start(group);
  const repo = new Repo();
  let tip: string | null = null;
  const log: unknown[] = [];
  for (let d = -40; d <= 0; d++) {
    await setDay(group, d, 1);
    const n = repo.commit(tip, `day ${d}`, [`f${d % 5}.txt`], d % 4 === 0 ? 12_000 : 300);
    await push(group, repo, "refs/heads/main", tip, n); tip = n;
    if (d === -36) await edge("/flags", { group, site: "c", flags: { offline: true } });
    await setDay(group, d, 3);
    await tick(group, log, `day ${d}`);
  }
  const before = (await edge<Views>("/view", { group })).map((v) => ({ site: v.status.site, base: v.status.base, applied: v.status.applied, snap: v.status.snapThrough }));
  await edge("/flags", { group, site: "c", flags: {} });
  const t = Date.now();
  await push(group, repo, "refs/heads/main", tip, repo.commit(tip, "rejoin", ["r.txt"], 100));
  let after: unknown = null;
  for (let i = 0; i < 20; i++) {
    await sleep(1500);
    const v = await edge<Views>("/view", { group });
    const c = v.find((x) => x.status.site === "c")!.status;
    if (c.installs > 0 && c.applied === v.find((x) => x.status.role === "leader")!.status.applied) { after = v.map((x) => ({ site: x.status.site, base: x.status.base, applied: x.status.applied, installs: x.status.installs })); break; }
  }
  const audit = await edge("/audit", { group });
  const v = await edge<Views>("/view", { group });
  const out = { group, before, after, rejoinMs: Date.now() - t, audit, cNotes: v.find((x) => x.status.site === "c")!.status.notes.slice(-10),
    pass: !!after && new Set(Object.values(audit.foldDigests)).size === 1 && audit.badTruncations.length === 0 };
  save(`wp8-${group}.json`, out);
  save(`wp8-${group}-ticks.json`, log);
  console.log(JSON.stringify({ pass: out.pass, before, after, rejoinMs: out.rejoinMs, fold: audit.foldDigests, bad: audit.badTruncations, cNotes: out.cNotes }));
}

/** Which time `ref_log` prunes by. A tag set on day -40 and deleted on day 1 was the tag's value on day 0; a restore to
 *  day 0 on day 30 needs it. The design group prunes by when a value was replaced; the `bywrite-` group by when it was
 *  written. Main gets a push every day so archiving, GC, and truncation run as in b028. */
async function window(group: string): Promise<void> {
  const out: Record<string, unknown> = {};
  for (const g of [group, `bywrite-${group}`]) {
    await start(g);
    const repo = new Repo();
    let main: string | null = null;
    const log: unknown[] = [];
    let tag: string | null = null;
    for (let d = -40; d <= 30; d++) {
      await setDay(g, d, 1);
      const n = repo.commit(main, `day ${d}`, [`f${d % 5}.txt`], 300);
      await push(g, repo, "refs/heads/main", main, n); main = n;
      if (d === -40) { tag = repo.commit(null, "tagged", ["t.txt"], 300); await push(g, repo, "refs/tags/v1", null, tag); }
      if (d === 1) await push(g, repo, "refs/tags/v1", tag, null);
      await setDay(g, d, 3);
      if (d % 2 === 0 || d > 25) await tick(g, log, `day ${d}`);
    }
    await setDay(g, 30, 8);
    const idx = await edge("/indexAt", { group: g, t: T0 + 4 * 3_600_000 });
    const restore = await edge("/restore", { group: g, rid: `restore-${g}`, to: idx.to, refs: ["refs/tags/v1"] });
    const refs = await edge("/refs", { group: g });
    out[g] = { tag, indexAt: idx, restore, refs, audit: await edge("/audit", { group: g }) };
    console.log(g, JSON.stringify({ restore: restore.out ?? restore, back: (refs as any[]).map((r) => r.refs?.["refs/tags/v1"] === tag) }));
  }
  save(`wp8-${group}.json`, out);
}

/** Negative test, on three ranges aged past LOG_KEEP in turn by moving the calendar.
 *  Range 1: N1 the leader's second put fails, so its HEAD sees one bucket and it commits nothing; N3 a defective leader
 *  commits `Archived` anyway without HEADing, and followers HEAD before truncating and refuse (repair off); N4 with repair
 *  on, a follower holding the committed entries re-puts the missing copy, HEADs both, and truncates.
 *  Range 2: N2 a leader deposed after both puts and before `Archived` (abandonAfterPut): the objects sit in both buckets
 *  but no record commits, so nobody truncates; then a new leader (b) archives the same range, whose content-addressed
 *  key is the same object, HEADs, commits, and members truncate.
 *  Range 3: N5 control for the audit: member c truncates by age alone (blindTruncate) while the leader's archive puts
 *  fail, and the edge's audit must flag it. */
async function negative(group: string): Promise<void> {
  const out: Record<string, unknown> = { group };
  await start(group);
  const repo = new Repo();
  let tip: string | null = null;
  const pushDay = async (d: number) => { await setDay(group, d, 1); const n = repo.commit(tip, `day ${d}`, [`f${d % 3}.txt`], 300); await push(group, repo, "refs/heads/main", tip, n); tip = n; };
  const views = async () => (await edge<Views>("/view", { group })).map((v) => ({ site: v.status.site, role: v.status.role, base: v.status.base, applied: v.status.applied, archThrough: v.status.archThrough, refused: v.status.refusedTruncations }));
  const flags = (site: string, f: Record<string, boolean>) => edge("/flags", { group, site, flags: f });
  for (let d = -40; d <= -17; d++) await pushDay(d);
  // Range 1: entries of days -40 to -33 are older than 30 days on day -2.
  await setDay(group, -2, 2);
  await flags("a", { skipSecondPut: true });
  out.n1_tick = await edge("/tick", { group });
  out.n1_views = await views();
  await flags("a", { skipSecondPut: true, commitWithoutHead: true, noRepair: true });
  for (const s of ["b", "c"]) await flags(s, { noRepair: true });
  out.n3_tick = await edge("/tick", { group });
  out.n3_views = await views();
  out.n3_audit = await edge("/audit", { group });
  await flags("a", { noRepair: true }); // a's own copy is the one present; keep the state until followers repair
  for (const s of ["b", "c"]) await flags(s, {});
  out.n4_tick = await edge("/tick", { group });
  out.n4_views = await views();
  out.n4_audit = await edge("/audit", { group });
  await flags("a", {});
  // Range 2: days -32 to -25 on day 6.
  await setDay(group, 6, 2);
  await flags("a", { abandonAfterPut: true });
  out.n2_tick = await edge("/tick", { group });
  out.n2_views = await views();
  await flags("a", {});
  out.n2_campaign = await edge("/campaign", { group, site: "b" });
  out.n2b_tick = await edge("/tick", { group });
  out.n2b_views = await views();
  out.n2b_audit = await edge("/audit", { group });
  // Range 3: days -24 to -17 on day 14, leader b's archive puts lose their second bucket, c truncates blind.
  await setDay(group, 14, 2);
  await flags("b", { skipSecondPut: true });
  await flags("c", { blindTruncate: true });
  out.n5_tick = await edge("/tick", { group });
  out.n5_views = await views();
  out.n5_audit = await edge("/audit", { group });
  for (const s of ["b", "c"]) await flags(s, {});
  const v = await edge<Views>("/view", { group });
  out.notes = v.map((x) => ({ site: x.status.site, notes: x.status.notes.slice(-40) }));
  save(`wp8-${group}.json`, out);
  const a = (k: string) => out[k] as { badTruncations: string[]; committedKeysNotInTwoBuckets: { key: string; head: unknown }[] };
  console.log(JSON.stringify({ n1: out.n1_views, n3: out.n3_views, n3bad: a("n3_audit").badTruncations, n3notInTwo: a("n3_audit").committedKeysNotInTwoBuckets.map((x) => x.head),
    n4: out.n4_views, n4bad: a("n4_audit").badTruncations, n4notInTwo: a("n4_audit").committedKeysNotInTwoBuckets.length,
    n2: out.n2_views, n2b: out.n2b_views, n2bbad: a("n2b_audit").badTruncations, n5: out.n5_views, n5bad: a("n5_audit").badTruncations }, null, 0));
  void iso;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { values, positionals } = parseArgs({ allowPositionals: true, strict: false, options: { group: { type: "string" } } });
  await main(positionals[0] ?? "", values.group as string | undefined);
}
