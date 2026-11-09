// L0-12's stall on m3org/j4: leases live before a stall of every member call, read after it.
//   1. N leases (half Exclusive on distinct tasks, half Shared), TTL 3,600 s; each lease's `expires` recorded.
//   2. Every member's `/status` `now` against this host's clock, three times: the member clock offsets.
//   3. Every member call on every site fails (`/_bed/fault?site=255`) for STALL minutes, every member running. Task
//      writes go out every 250 ms across the cutover so the leader has an entry in flight that no quorum
//      acknowledges (`tick_leader`'s marker); followers' `suspect` reports add theirs.
//   4. The fault clears. The next write carries `extend`; then every lease is read: live, with `expires` moved by the
//      recorded stall (`stalledSince` from `/status` before it clears), and clock offsets again.
//   5. An idle 60 s Shared lease, then 10 minutes later an overlapping Exclusive acquire, which must succeed.
// With `change` (am1-fix), the stall ends in a leader change: once the fault lapses and a follower holds the old
// leader's last index (its re-send), the old leader transfers to that follower (`/transfer`), unless an election
// already moved the lead. The entry that carried `extend`, and its term, are read from the new leader's `/log`.
// Rows go to tests/am1/raw/stall.jsonl.
//   node tests/am1/hosted/stall.mjs [minutes=30] [n=20] [change]
import { api, bedGet, ghs, log, logFrom, member, row, sleep, state, statuses, tokens } from "./lib.mjs";

const STALL = Number(process.argv[2] ?? 30);
const N = Number(process.argv[3] ?? 20);
const CHANGE = process.argv[4] === "change";
const OUT = new URL("../raw/stall.jsonl", import.meta.url).pathname;
// j4 by default; `STALL_REPO`/`STALL_REPO_ID` name another m3org repository the m3-ci installation covers.
const R = `m3org/${process.env.STALL_REPO ?? "j4"}`;
const V1 = `/api/graftyard/v1/repos/${R}`;
const CHAIN = `repo:${process.env.STALL_REPO_ID ?? state.repo}`;
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
  if (l.status === 201) leases.push({ id: l.body.lease.id, expires: l.body.lease.expires_at, mode: l.body.lease.mode, task: t.body.id });
  else step("acquire refused", { i, status: l.status, body: l.body });
}
step("leases before", { n: leases.length, sample: leases.slice(0, 2) });
step("clock offsets before", { offsets: await offsets() });
step("status before", { members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, commit: s.commit, stalledSince: s.stalledSince })) });

// The fault takes effect AHEAD_MS from now (members reread it at most once a second), while task writes go out every
// 250 ms across the cutover: a write sent after the fault never reaches `decide` (the leader's `fresh` read of its
// deps fails first), so the leader holds an entry in flight only if one is appended just before replication fails.
const AHEAD_MS = 3_000;
const fault = await bedGet(`/_bed/fault?site=255&after_ms=${AHEAD_MS}&for_ms=${STALL * 60_000}`);
step("fault armed", { status: fault.status, aheadMs: AHEAD_MS, body: fault.body.slice(0, 300) });
const tFault = Date.now() + AHEAD_MS;
const writes = [];
for (let k = 0; Date.now() < tFault + 3_000; k++) {
  writes.push(api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-w${k}`, summary: "across the cutover" }).then((r) => r.status));
  await sleep(250);
}
const inflight = Promise.all(writes).then((st) => ({ status: st.reduce((m, x) => ((m[x] = (m[x] ?? 0) + 1), m), {}) }));
step("cutover writes sent", { n: writes.length, members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, last: s.last, commit: s.commit, stalledSince: s.stalledSince })) });
const brief = (st) => st.map((s) => ({ site: s.site, role: s.role, term: s.term, last: s.last, commit: s.commit, stalledSince: s.stalledSince }));
let before = null;
while (Date.now() - tFault < STALL * 60_000) {
  await sleep(Math.min(60_000, Math.max(0, tFault + STALL * 60_000 - Date.now())));
  const st = await statuses(CHAIN);
  before = st;
  step("during", { min: Math.round((Date.now() - tFault) / 60_000), members: brief(st) });
}
if (CHANGE) {
  // The fault lapsed on its own just now. Wait for a follower to hold the old leader's last index, then hand off.
  const old = before.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
  let moved = null;
  for (let i = 0; i < 100 && old; i++) {
    const st = await statuses(CHAIN);
    const lead = st.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
    if (lead && lead.term > old.term) { moved = { by: "election", members: brief(st) }; break; }
    const f = st.find((s) => s.role === "follower" && s.last >= old.last);
    if (f) {
      const t = await member(CHAIN, old.site, "/transfer", { to: f.site }, 15_000).catch((e) => ({ error: String(e).slice(0, 120) }));
      await sleep(3_000);
      moved = { by: "transfer", to: f.site, transfer: t.v ?? t, members: brief(await statuses(CHAIN)) };
      break;
    }
    await sleep(300);
  }
  step("leader change", { old: old && { site: old.site, term: old.term, last: old.last, stalledSince: old.stalledSince }, ...moved });
}
const off = await bedGet("/_bed/fault?off=1");
step("fault off", { status: off.status, stalledMs: Date.now() - tFault });
step("cutover write answers", await inflight);
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
  // The API's fields are `expires_at` and `granted_at` (whole seconds); each lease's TTL was 3,600 s.
  return { id: l.id, live: !!a, movedMs: a ? Date.parse(a.expires_at) - Date.parse(a.granted_at) - 3_600_000 : null, mode: a?.mode };
});
step("leases after", { status: after.status, live: moved.filter((m) => m.live).length, of: leases.length, moved: moved.map((m) => m.movedMs) });
step("status after write", { members: (await statuses(CHAIN)).map((s) => ({ site: s.site, role: s.role, term: s.term, stalledSince: s.stalledSince })) });
// Which entries after the stall's start carried `extend`, in which term (test-faults `/log` on the leader).
const from = Math.max(1, (before?.reduce((m, s) => Math.min(m, s.commit ?? m), Infinity) ?? 1) - 2);
const entries = await logFrom(CHAIN, Number.isFinite(from) ? from : 1, 50);
step("entries after the stall", { entries: entries.map((e) => ({ index: e.index, term: e.term, at: e.at, kind: e.kind, extend: e.extend ?? null })) });
step("clock offsets after", { offsets: await offsets() });

// The idle 60 s lease, then 10 minutes later an overlapping Exclusive acquire by another task.
const t1 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-idle`, summary: "idle" });
const idle = await api("POST", `${V1}/leases`, app, { paths: [`idle/${tag}/**`], mode: "exclusive", ttl_seconds: 60 }, { "x-graftyard-agent-session": t1.body?.id ?? "" });
step("idle 60 s exclusive", { status: idle.status, expires: idle.body?.lease?.expires_at });
await sleep(10 * 60_000);
app = (await ghs({})).body.token;
const t2 = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `stall-${tag}-late`, summary: "late" });
const late = await api("POST", `${V1}/leases`, app, { paths: [`idle/${tag}/**`], mode: "exclusive", ttl_seconds: 600 }, { "x-graftyard-agent-session": t2.body?.id ?? "" });
step("late overlapping exclusive", { status: late.status, conflicts: late.body?.conflicts?.length ?? 0 });
