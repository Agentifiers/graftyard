// F67's hosted proof on graft-m3c: a loaded repository group whose leader goes half-hung (every answer to its
// AppendEntries lost after the follower ran it: the member route `/sever` with `half`) must elect a new leader with no
// `/campaign`, and a request must complete within the 30 s bound (`tests/m0/results/f67.md`).
//   1. A fresh m3org repository (`mkrepos.mjs` NAME first); its group's leader and members' `/status`.
//   2. Load: task writes every 500 ms and REST reads of the repository every 250 ms, through the edge, for LOAD_S.
//   3. At FAULT_AT_S the leader is half-hung for HALF_S. Every second, each member's role and term.
//   4. When a member other than the old leader leads in a higher term, and when the first write after the fault
//      completes. The fault is lifted at the end in any case.
// Rows to tests/m0/f67/raw/<tag>.jsonl.
//   node tests/m0/f67/hosted.mjs NAME REPO_ID [load_s=120] [fault_at_s=30] [half_s=60]
import { mkdirSync } from "node:fs";
import { api, ghs, log, member, row, sleep, statuses } from "../../am1/hosted/lib.mjs";

const [NAME, RID] = process.argv.slice(2, 4);
const LOAD_S = Number(process.argv[4] ?? 120);
const FAULT_AT_S = Number(process.argv[5] ?? 30);
const HALF_S = Number(process.argv[6] ?? 60);
if (!NAME || !RID) throw new Error("usage: node tests/m0/f67/hosted.mjs NAME REPO_ID [load_s] [fault_at_s] [half_s]");
const CHAIN = `repo:${RID}`;
const V1 = `/api/graftyard/v1/repos/m3org/${NAME}`;
const tag = `f67-${Date.now().toString(36)}`;
mkdirSync(new URL("./raw/", import.meta.url), { recursive: true });
const OUT = new URL(`./raw/${tag}.jsonl`, import.meta.url).pathname;
const step = (name, v) => { row(OUT, { step: name, at: Date.now(), ...v }); log(`${name} ${JSON.stringify(v).slice(0, 500)}`); };
const brief = (st) => st.map((s) => ({ site: s.site, role: s.role, term: s.term, last: s.last, commit: s.commit, leader: s.leader }));

let app = (await ghs({})).body.token;
// A first write creates and elects the group.
const w0 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `${tag}-w0`, summary: "F67 first write" });
step("first write", { status: w0.status, ms: w0.ms });
let st = await statuses(CHAIN);
const lead0 = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
step("before", { members: brief(st) });
if (!lead0) throw new Error("no leader before the fault");

const t0 = Date.now();
const tFault = t0 + FAULT_AT_S * 1_000;
const tEnd = t0 + LOAD_S * 1_000;
const done = []; // {kind, sent, ms, status}
let stop = false;
async function writer() {
  for (let k = 0; !stop; k++) {
    const sent = Date.now();
    api("POST", `${V1}/tasks`, app, { kind: "external", session: `${tag}-w${k}`, summary: "F67 load" })
      .then((r) => done.push({ kind: "w", sent, end: Date.now(), status: r.status }));
    await sleep(500);
  }
}
async function reader() {
  while (!stop) {
    const sent = Date.now();
    api("GET", `/api/v3/repos/m3org/${NAME}`, app).then((r) => done.push({ kind: "r", sent, end: Date.now(), status: r.status }));
    await sleep(250);
  }
}
const loops = [writer(), reader()];

let faulted = null;
let moved = null;
const timeline = [];
while (Date.now() < tEnd) {
  await sleep(1_000);
  if (!faulted && Date.now() >= tFault) {
    const r = await member(CHAIN, lead0.site, "/sever", { for_ms: HALF_S * 1_000, half: true }, 15_000);
    faulted = { at: Date.now(), site: lead0.site, term: lead0.term, answer: r.v ?? r };
    step("half-hung", faulted);
  }
  st = await statuses(CHAIN);
  const lead = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  timeline.push({ t: Date.now() - t0, members: st.map((s) => [s.site, s.role?.[0], s.term]) });
  if (faulted && !moved && lead && lead.site !== lead0.site && lead.term > lead0.term) {
    moved = { at: Date.now(), afterFaultMs: Date.now() - faulted.at, site: lead.site, term: lead.term };
    step("new leader", moved);
  }
}
stop = true;
await Promise.all(loops);
await sleep(15_000);
const lift = await member(CHAIN, lead0.site, "/sever", { for_ms: 0, half: true }, 15_000).catch((e) => ({ error: String(e) }));
step("lifted", { answer: lift.v ?? lift });

// Requests sent after the fault: the first that completed with 2xx, and the longest gap with none completing.
const ok = (d) => d.status >= 200 && d.status < 300;
const after = done.filter((d) => faulted && d.sent >= faulted.at).sort((a, b) => a.end - b.end);
const firstOk = after.find(ok);
const okEnds = done.filter(ok).map((d) => d.end).sort((a, b) => a - b);
let gap = 0, gapFrom = 0;
for (let i = 1; i < okEnds.length; i++) if (okEnds[i] - okEnds[i - 1] > gap) { gap = okEnds[i] - okEnds[i - 1]; gapFrom = okEnds[i - 1]; }
const byStatus = (k, from, to) => done.filter((d) => d.kind === k && d.sent >= from && d.sent < to)
  .reduce((m, d) => ((m[d.status] = (m[d.status] ?? 0) + 1), m), {});
step("result", {
  oldLeader: { site: lead0.site, term: lead0.term },
  newLeader: moved,
  firstOkAfterFaultMs: firstOk && faulted ? firstOk.end - faulted.at : null,
  longestGapMs: gap, gapFromFaultMs: faulted ? gapFrom - faulted.at : null,
  writesBefore: byStatus("w", t0, faulted?.at ?? tEnd), writesAfter: byStatus("w", faulted?.at ?? tEnd, tEnd),
  readsBefore: byStatus("r", t0, faulted?.at ?? tEnd), readsAfter: byStatus("r", faulted?.at ?? tEnd, tEnd),
  campaign: "none sent",
});
step("timeline", { timeline });
step("after", { members: brief(await statuses(CHAIN)) });
