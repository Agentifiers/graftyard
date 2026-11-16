// LM-1 round 16b: every voter's and learner's `/stats` (raw, so `raftIn`, `trace` and `inbound`) of one repository
// group every `every` seconds, for the near follower's AppendEntries split and each member's read share. Read only.
//   node tests/fanout/legs.mjs <repo name> <seconds> <every> <out dir>
import { readFileSync, writeFileSync } from "node:fs";
import { member, voterNames, statuses, sleep } from "../am1/hosted/lib.mjs";
const [name, secs, every, dir] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
const idOf = () => readFileSync(`${process.env.M3_STATE}/am1-runs-repos.txt`, "utf8").split("\n").map((l) => l.split(" ")).find(([n]) => n === name)?.[1];
let id;
while (!(id = idOf()) && Date.now() < until) await sleep(2_000);
const chain = `repo:${id}`;
let names = {}, learners = [];
for (let k = 0; Date.now() < until; k++) {
  const t = Date.now();
  const got = await voterNames(chain).catch(() => ({}));
  if (Object.keys(got).length) names = got;
  if (!learners.length) {
    const st = await statuses(chain).catch(() => []);
    const ls = st.find((s) => s.role === "leader")?.config?.config?.normal?.learners ?? [];
    learners = ls.map((l) => ({ site: l.site, name: l.do_name }));
  }
  // The leader's raw `/stats` is the one heavy read (round 6: a loaded leader's raw arrays rebuilt its isolate), so
  // it is taken every third sample only; its trace ring holds about 250 s of appends at B0-29's rate.
  const lead = (await statuses(chain).catch(() => [])).find((s) => s.role === "leader")?.site;
  const who = [...[0, 1, 2].filter((s) => s !== lead || k % 3 === 0).map((s) => ({ site: s, name: names[s], voter: true })), ...learners];
  const out = await Promise.all(who.map(async (w) => {
    const r = await member(chain, w.site, "/stats", undefined, 20_000, w.name).catch((e) => ({ status: 0, v: { error: String(e).slice(0, 80) } }));
    const v = r.v ?? {};
    return { site: w.site, name: w.name ?? null, voter: !!w.voter, role: v.role ?? null, inbound: v.inbound ?? null,
      raftIn: v.raftIn ?? null, trace: (v.trace ?? []).filter((x) => [8, 10, 12, 13, 14, 15, 16, 17, 20, 21].includes(x[1])),
      confirmMs: v.confirmMs?.length, error: v.error };
  }));
  // The leader's memory split (narrow-reads-3's `memSplit`, `logBytes`, `held`) from the light summary, every sample.
  if (lead !== undefined) {
    const sm = (await member(chain, lead, "/stats?summary=1", undefined, 15_000, names[lead]).catch(() => ({}))).v ?? {};
    out.push({ site: lead, summary: true, wasmMemMb: sm.wasmMemMb, logBytes: sm.logBytes, memSplit: sm.memSplit, held: sm.held,
      entries: sm.entries, commands: sm.commands });
  }
  writeFileSync(`${dir}/legs-${String(k).padStart(3, "0")}.json`, JSON.stringify({ at: new Date(t).toISOString(), chain, members: out }));
  await sleep(Math.max(0, Number(every) * 1000 - (Date.now() - t)));
}
