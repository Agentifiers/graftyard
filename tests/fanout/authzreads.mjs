// merge-front's leader-read flip (`decisions/DECISIONS.md`, "`merge-front`"): the owner `Authz` group's leader `/read`
// count by kind and its shed reads, sampled every `every` seconds for `secs`, one JSON line each, so a row reads the
// merges' added leader reads and any READS_MAX shed against the load window. Read only.
//   node tests/fanout/authzreads.mjs <chain, e.g. authz:7100001> <secs> <every>   -> stdout
import { member, statuses, voterNames, sleep } from "../am1/hosted/lib.mjs";
const [chain, secs, every] = process.argv.slice(2);
const until = Date.now() + Number(secs) * 1000;
while (Date.now() < until) {
  const t = Date.now();
  const lead = (await statuses(chain).catch(() => [])).find((s) => s.role === "leader")?.site;
  let out = { at: new Date(t).toISOString(), chain, leader: lead ?? null };
  if (lead !== undefined) {
    const names = await voterNames(chain).catch(() => ({}));
    const v = (await member(chain, lead, "/stats?summary=1", undefined, 15_000, names[lead]).catch((e) => ({ v: { error: String(e).slice(0, 80) } }))).v ?? {};
    out = { ...out, inboundMs: v.inboundMs ?? null, readsBy: v.readsBy ?? null, shedReads: v.shedReads ?? null, shedBy: v.shedBy ?? null, error: v.error };
  }
  console.log(JSON.stringify(out));
  await sleep(Math.max(0, Number(every) * 1000 - (Date.now() - t)));
}
