// G6 (decisions/OPEN-QUESTIONS.md): race GC's `Compacted` against a push and a `RestoredTo` that name objects GC is
// expiring at the 14-day cruft line, on the graft-wp8 bed (src/wp8/).
//
//   node run/wp8-race.ts --group g6-on-1 --trials 40         design roots, all guards on
//   node run/wp8-race.ts --group g6-nocg-1 --trials 40       `Compacted` guard off (control)
//   node run/wp8-race.ts --group noreflog-g6-on-1 ...        roots without `ref_log`, so restore targets expire too
//   (`-nopg-` and `-norg-` in a group name turn off the push and `RestoredTo` guards; see `guardsOf` in src/wp8/types.ts)
//
// One trial, all on the simulated calendar:
//  1. Seed day S: push branches p<n> and r<n>, each one commit with a 12 KB blob (a stored pack). Day S+1: delete both.
//  2. The objects leave the root set when nothing roots them: with `ref_log` roots, when the deleted values are pruned,
//     35 days after the delete; without, at the delete. One day after that, GC moves them to cruft with `since` = then.
//  3. Race day, 14 days and a minute after the cruft GC, when they are due to expire. Started together with random delays:
//     GC (reads roots and packs, places its packs, waits a random hold, submits `Compacted`); (a) a push that re-points
//     p<n> at its deleted commit and sends no objects (connectivity read of the live packs, random hold, submit); and (b)
//     a `RestoredTo` of r<n>. With `ref_log` roots (b) targets a value set the day before and deleted, which GC keeps;
//     without, it targets the seeded value, whose objects GC is expiring.
//  4. Refused commands are retried as a client would, each with a fresh request id after re-reading. A refusal for
//     missing objects is final. GC handles its own reruns (`gcRace` in src/wp8/member.ts).
//  5. Checks: every object reachable from `refs` (and, with `ref_log` roots, from `ref_log`) is in a live pack in R2; a
//     push or restore that ended refused for missing objects left its ref unset; one that committed has its objects.

import { parseArgs } from "node:util";
import { DAY, edge, Repo, save, sleep } from "./wp8.ts";

const { values: opt } = parseArgs({ strict: false, options: { group: { type: "string" }, trials: { type: "string" } } });
const group = (opt.group as string) ?? `g6-on-${Date.now().toString(36)}`;
const TRIALS = Number(opt.trials ?? 30);
const NOREFLOG = group.startsWith("noreflog-");
const LEAVE = NOREFLOG ? 1 : 36; // days after the seed when the deleted values stop being roots
const T0 = Date.parse("2027-01-01T12:00:00Z");
const rnd = (a: number, b: number) => a + Math.floor(Math.random() * (b - a + 1));
const MISSING = /objects missing|no longer holds|outside/;

async function setCal(t: number) { await edge("/clock", { group, cal: t }); }
/** Re-sends with a fresh request id until accepted, or refused for a reason no retry fixes. */
async function retrying(send: () => Promise<any>) {
  const refusals: string[] = [];
  for (let i = 1; i <= 10; i++) {
    const r = await send().catch((e) => ({ error: String(e).slice(0, 160) }));
    if (r.out?.ok) return { tries: i, ok: true, entry: r.entry as number, refusals };
    const why = r.out?.why ?? r.error ?? JSON.stringify(r).slice(0, 120);
    refusals.push(why);
    if (MISSING.test(why) || /stale/.test(why)) return { tries: i, ok: false, entry: 0, refusals };
    await sleep(rnd(20, 200));
  }
  return { tries: 10, ok: false, entry: 0, refusals, unsettled: true };
}

async function main() {
  const trials: unknown[] = [];
  let cal = T0;
  await setCal(cal);
  for (let i = 0; i < 5; i++) { const r = await edge("/campaign", { group, site: "a" }); if (/leader/.test(r.result)) break; await sleep(1000); }
  const repo = new Repo();
  const push = async (ref: string, old: string | null, tip: string | null) => {
    const r = await edge("/push", { group, rid: crypto.randomUUID(), updates: ref ? [{ ref, old, new: tip }] : [], objects: tip ? repo.objects(tip) : [] });
    if (!r.out?.ok) throw new Error(`seed push ${ref}: ${JSON.stringify(r).slice(0, 200)}`);
    return r.entry as number;
  };
  let main = repo.commit(null, "root", ["m.txt"], 12_000);
  await push("refs/heads/main", null, main);
  for (let n = 0; n < TRIALS; n++) {
    const S = cal = cal + 70 * DAY; await setCal(S);
    const p = repo.commit(main, `P ${n}`, [`p/${n}.txt`], 12_000), r = repo.commit(main, `R ${n}`, [`r/${n}.txt`], 12_000);
    await push(`refs/heads/p${n}`, null, p);
    const rTo = await push(`refs/heads/r${n}`, null, r);
    await setCal(S + DAY);
    await push(`refs/heads/p${n}`, p, null);
    await push(`refs/heads/r${n}`, r, null);
    const next = repo.commit(main, `main ${n}`, ["m.txt"], 12_000); await push("refs/heads/main", main, next); main = next;
    // Cruft GC one day after the values stop being roots; an empty entry at that time prunes `ref_log` first.
    const C = S + (LEAVE + 1) * DAY; await setCal(C);
    await push("", null, null);
    const cruftGc = await edge("/gcRace", { group, holdMs: 0 });
    // With `ref_log` roots, the restore targets a value inside the window: r<n> set to a new commit the day before the
    // race and deleted; its objects are roots, so this races the restore's walk against packs GC is consuming.
    const R = C + 14 * DAY + 60_000;
    let restoreTo = rTo, restoreTip = r;
    if (!NOREFLOG) {
      await setCal(R - DAY);
      restoreTip = repo.commit(main, `RB ${n}`, [`rb/${n}.txt`], 12_000);
      restoreTo = await push(`refs/heads/r${n}`, null, restoreTip);
      await push(`refs/heads/r${n}`, restoreTip, null);
    }
    await setCal(R);
    await push("", null, null);
    const plan = await edge("/gcPlan", { group });
    // GC's own work (pack reads and two placements) takes about 2 s before its hold; the delays span both sides of it.
    const holds = { gc: rnd(0, 2500), push: rnd(0, 800), restore: rnd(0, 800), pushDelay: rnd(0, 4000), restoreDelay: rnd(0, 4000) };
    const t0 = Date.now();
    const [gc, pu, re] = await Promise.all([
      edge("/gcRace", { group, holdMs: holds.gc }).catch((e) => ({ error: String(e) })),
      sleep(holds.pushDelay).then(() => retrying(() => edge("/pushExisting", { group, rid: crypto.randomUUID(), updates: [{ ref: `refs/heads/p${n}`, old: null, new: p }], holdMs: holds.push }))),
      sleep(holds.restoreDelay).then(() => retrying(() => edge("/restore", { group, rid: crypto.randomUUID(), to: restoreTo, refs: [`refs/heads/r${n}`], holdMs: holds.restore }))),
    ]);
    const raceMs = Date.now() - t0;
    await sleep(300);
    const lost = await edge("/lost", { group, oids: [p, restoreTip] });
    const refs = (await edge("/refs", { group })).find((x: any) => x.leader)?.refs ?? {};
    const why: string[] = [];
    if (lost.refsMissing.length) why.push(`refs reach ${lost.refsMissing.length} missing objects`);
    if (!NOREFLOG && lost.reflogMissing.length) why.push(`ref_log reaches ${lost.reflogMissing.length} missing objects`);
    if ((pu as any).unsettled) why.push("push never settled");
    if ((re as any).unsettled) why.push("restore never settled");
    if (!pu.ok && refs[`refs/heads/p${n}`]) why.push("push refused but p set");
    if (!re.ok && refs[`refs/heads/r${n}`]) why.push("restore refused but r set");
    if (pu.ok && !lost.present[p]) why.push("push committed, p missing");
    if (re.ok && !lost.present[restoreTip]) why.push("restore committed, target missing");
    // Commit order by entry index: G = Compacted, P = push, R = restore; lower-case = refused in the end.
    const ev = [["G", (gc as any).entry || Infinity], ["P", pu.entry || Infinity], ["R", re.entry || Infinity]] as [string, number][];
    const order = ev.sort((a, b) => a[1] - b[1]).map(([k, i]) => (i === Infinity ? k.toLowerCase() : k)).join("");
    const trial = { n, holds, raceMs, expiringAtStart: plan.expiring.length, pExpiring: plan.expiring.includes(p), targetExpiring: plan.expiring.includes(restoreTip),
      cruftGc: { committed: cruftGc.committed, attempts: cruftGc.attempts }, gc, push: pu, restore: re, order, lost, pass: why.length === 0, why };
    trials.push(trial);
    console.log(`${group} ${n}: ${trial.pass ? "ok  " : "FAIL"} ${order} gc att ${(gc as any).attempts} cheap ${(gc as any).cheapReruns} full ${(gc as any).fullReruns} exp ${(gc as any).expired} | push ${pu.ok ? "ok" : pu.refusals.at(-1)?.slice(0, 40)} x${pu.tries} | restore ${re.ok ? "ok" : re.refusals.at(-1)?.slice(0, 40)} x${re.tries} ${why.join("; ")}`);
    save(`wp8-${group}.json`, { group, noreflog: NOREFLOG, trials });
  }
}
await main();
