// narrow-reads-3: splits each learner's read-index grant wait, per ReadIndex batch it sent, from the test-faults
// `/stats` trace (`workers/site/src/host.rs`, `Trace`):
//   (i)   rtt:  IndexSent (1) to the leader's ReadIndexRes stepped here on its own call (3, ms 0), read index R;
//   (ii)  hold: that response to the first AppendEntries stepped here (17) that brings this member both entries through
//         R and a commit at or past R; 0 when it already had both. `via` says where that append came from: `same` in
//         the response that carried the ReadIndexRes, `own` in a later response to this member's own call, `call` in a
//         call from the leader, `before` already held;
//   (iii) apply: from the later of those two to the applied index (4) reaching R.
// The leader's side, per learner: how long its AppendEntries waited held for the learner's next call (18 to 19), by
// round. Confirm (10) and Stepped (15) times on each learner give the storage confirm and turn time behind (iii).
// p50/p99/max in ms.
//   node tests/nr3/split.mjs repo:ID
const { member, statuses } = await import("../am1/hosted/lib.mjs");
const chain = process.argv[2];
const st = await statuses(chain);
const cfg = st.find((s) => s.config)?.config?.config?.normal;
const learners = (cfg?.learners ?? []).map((l, i) => [l.site, l.do_name, `l${i}`]);
const leader = st.find((s) => s.role === "Leader")?.site;
const pct = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const at = (p) => (s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null);
  return { n: s.length, p50: at(0.5), p99: at(0.99), max: s.length ? s[s.length - 1] : null };
};
const SAME = 2; // ms: a ReadIndexRes and an append stepped from one response land in the same turn.
for (const [site, name, tag] of learners) {
  try {
    const v = (await member(chain, site, "/stats", undefined, 30000, name)).v ?? {};
    const tr = v.trace ?? [];
    const sent = new Map();
    // Per answered batch: [R, t1, t2, via] until applied.
    const open = [];
    const rtt = [], hold = [], apply = [], total = [];
    const via = { same: 0, own: 0, call: 0, before: 0 };
    let last = 0, commit = 0, applied = 0, lastRes = -1e9;
    const confirm = [], stepped = [];
    for (const [at, kind, , batch, index, ms] of tr) {
      if (kind === 1) sent.set(batch, at);
      else if (kind === 3 && ms === 0 && sent.has(batch)) {
        const t0 = sent.get(batch);
        sent.delete(batch);
        if (!index) continue;
        rtt.push(at - t0);
        lastRes = at;
        const o = { R: index, t0, t1: at, t2: null, via: null };
        if (last >= index && commit >= index) [o.t2, o.via] = [at, "before"];
        open.push(o);
      } else if (kind === 17) {
        last = Math.max(last, index);
        commit = Math.max(commit, ms);
        for (const o of open)
          if (o.t2 === null && last >= o.R && commit >= o.R)
            [o.t2, o.via] = [at, batch === 1 ? "call" : at - o.t1 <= SAME ? "same" : "own"];
      } else if (kind === 4) {
        applied = index;
        for (let k = open.length - 1; k >= 0; k--) {
          const o = open[k];
          if (o.R > applied) continue;
          const t2 = o.t2 ?? at;
          hold.push(Math.max(0, t2 - o.t1));
          apply.push(at - Math.max(o.t1, t2));
          total.push(at - o.t0);
          via[o.via ?? "before"]++;
          open.splice(k, 1);
        }
      } else if (kind === 10) confirm.push(ms);
      else if (kind === 15) stepped.push(ms);
    }
    console.log(
      tag,
      site,
      JSON.stringify({ role: v.role, entries: tr.length, from: tr[0]?.[0], rtt: pct(rtt), hold: pct(hold), apply: pct(apply), total: pct(total), via, unapplied: open.length, confirm: pct(confirm), stepped: pct(stepped) }),
    );
  } catch (e) {
    console.log(tag, String(e).slice(0, 80));
  }
}
try {
  const v = (await member(chain, leader, "/stats", undefined, 30000)).v ?? {};
  const held = new Map(), wait = new Map(), n = new Map();
  for (const [at, kind, peer, batch] of v.trace ?? []) {
    if (kind === 18) held.set(`${peer}/${batch}`, at);
    else if (kind === 19 && held.has(`${peer}/${batch}`)) {
      if (!wait.has(peer)) wait.set(peer, []);
      wait.get(peer).push(at - held.get(`${peer}/${batch}`));
      held.delete(`${peer}/${batch}`);
    }
    if (kind === 18) n.set(peer, (n.get(peer) ?? 0) + 1);
  }
  for (const [peer, w] of [...wait].sort((a, b) => a[0] - b[0]))
    console.log(`leader f${leader} held for ${peer}`, JSON.stringify({ held: n.get(peer), wait: pct(w) }));
} catch (e) {
  console.log("leader", String(e).slice(0, 80));
}
