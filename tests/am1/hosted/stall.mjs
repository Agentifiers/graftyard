// L0-12's stall on m3org/j4: leases live before a stall of every member call, read after it.
//   1. N leases (half Exclusive on distinct tasks, half Shared), TTL 3,600 s; each lease's `expires` recorded.
//   2. Every member's `/status` `now` against this host's clock, three times: the member clock offsets.
//   3. Every member call on every site fails (`/_bed/fault?site=255`) for STALL minutes, every member running. A
//      push is sent at the start so the leader has an entry in flight that no quorum acknowledges (`tick_leader`'s
//      marker); followers' `suspect` reports add theirs.
//   4. The fault clears. The next write carries `extend`; then every lease is read: live, with `expires` moved by the
//      recorded stall (`stalledSince` from `/status` before it clears), and clock offsets again.
//   5. An idle 60 s Shared lease, then 10 minutes later an overlapping Exclusive acquire, which must succeed.
// Rows go to tests/am1/raw/stall.jsonl.
//   node tests/am1/hosted/stall.mjs [minutes=30] [n=20]
import { api, bedGet, ghs, log, row, sleep, state, statuses, tokens } from "./lib.mjs";

const STALL = Number(process.argv[2] ?? 30);
const N = Number(process.argv[3] ?? 20);
const OUT = new URL("../raw/stall.jsonl", import.meta.url).pathname;
const R = "m3org/j4";
const V1 = `/api/graftyard/v1/repos/${R}`;
const CHAIN = `repo:${state.repo}`;
const tag = Date.now().toString(36);
const step = (name, v) => { row(OUT, { step: name, at: Date.now(), ...v }); log(`${name} ${JSON.stringify(v).slice(0, 400)}`); };

async function offsets() {
  const out = [];
  for (let k = 0; k < 3; k++) {
    const t0 = Date.now();
    const st = await statuses(CHAIN);
    const t1 = Date.now();
    // The member's clock against the midpoint of the round trip; the half round trip bounds the error.
    for (const s of st) if (s.now) out.push({ site: s.site, offsetMs: s.now - (t0 + t1) / 2, halfRttMs: (t1 - t0) / 2 });
  }
  return out;
}

let app = (await ghs({})).body.token;
const leases = [];
for (let i = 0; i < N; i++) {
  const ex = i % 2 === 0;
  const t = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-${i}`, summary: "L0-12 stall" });
  const l = await api("POST", `${V1}/leases`, app, { paths: [`stall/${tag}/${i}/**`], mode: ex ? "exclusive" : "shared", ttl_seconds: 3600, intent: "L0-12" },
    { "x-graftyard-agent-session": t.body?.id ?? "" });
  if (l.status === 201) leases.push({ id: l.body.lease.id, expires: l.body.lease.expires, mode: l.body.lease.mode, task: t.body.id });
  else step("acquire refused", { i, status: l.status, body: l.body });
}
step("leases before", { n: leases.length, sample: leases.slice(0, 2) });
step("clock offsets before", { offsets: await offsets() });
step("status before", { members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, commit: s.commit, stalledSince: s.stalledSince })) });

const fault = await bedGet(`/_bed/fault?site=255&for_ms=${STALL * 60_000}`);
step("fault on", { status: fault.status, body: fault.body.slice(0, 300) });
const tFault = Date.now();
// An entry in flight that no quorum acknowledges: a task write the leader takes and cannot commit.
const inflight = api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-inflight`, summary: "in flight" });
while (Date.now() - tFault < STALL * 60_000) {
  await sleep(60_000);
  const st = await statuses(CHAIN);
  step("during", { min: Math.round((Date.now() - tFault) / 60_000), members: st.map((s) => ({ site: s.site, role: s.role, term: s.term, commit: s.commit, stalledSince: s.stalledSince })) });
}
const off = await bedGet("/_bed/fault?off=1");
step("fault off", { status: off.status, stalledMs: Date.now() - tFault });
step("inflight answer", { status: (await inflight).status });
step("status after", { members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, commit: s.commit, stalledSince: s.stalledSince })) });
// The next write carries `extend`.
app = (await ghs({})).body.token;
const w = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-after`, summary: "after the stall" });
step("first write after", { status: w.status });
await sleep(5_000);
const after = await api("GET", `${V1}/leases?per_page=100&paths=stall/${tag}/**`, tokens.admin);
const byId = new Map((Array.isArray(after.body) ? after.body : []).map((l) => [l.id, l]));
const moved = leases.map((l) => {
  const a = byId.get(l.id);
  return { id: l.id, live: !!a, movedMs: a ? Date.parse(a.expires) - Date.parse(l.expires) : null, mode: a?.mode };
});
step("leases after", { status: after.status, live: moved.filter((m) => m.live).length, of: leases.length, moved: moved.map((m) => m.movedMs) });
step("status after write", { members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, stalledSince: s.stalledSince })) });
step("clock offsets after", { offsets: await offsets() });

// The idle 60 s lease, then 10 minutes later an overlapping Exclusive acquire by another task.
const t1 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-idle`, summary: "idle" });
const idle = await api("POST", `${V1}/leases`, app, { paths: [`idle/${tag}/**`], mode: "exclusive", ttl_seconds: 60 }, { "x-graftyard-agent-session": t1.body?.id ?? "" });
step("idle 60 s exclusive", { status: idle.status, expires: idle.body?.lease?.expires });
await sleep(10 * 60_000);
app = (await ghs({})).body.token;
const t2 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-late`, summary: "late" });
const late = await api("POST", `${V1}/leases`, app, { paths: [`idle/${tag}/**`], mode: "exclusive", ttl_seconds: 600 }, { "x-graftyard-agent-session": t2.body?.id ?? "" });
step("late overlapping exclusive", { status: late.status, conflicts: late.body?.conflicts?.length ?? 0 });
