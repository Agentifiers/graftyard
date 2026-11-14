// narrow-reads-3: where a read-index grant's time goes, per member, from the test-faults `/stats` trace
// (`workers/site/src/host.rs`, `Trace`): for each `ReadIndex` batch this member sent, `rtt` from IndexSent (1) to the
// leader's ReadIndexRes stepped here (3, own call), and `apply` from that to this member's applied index (4) reaching
// the batch's read index. Also the batches the leader never answered. p50/p99/max in ms.
//   node tests/nr3/trace.mjs repo:ID
const { member, statuses } = await import("../am1/hosted/lib.mjs");
const chain = process.argv[2];
const st = await statuses(chain);
const cfg = st.find((s) => s.config)?.config?.config?.normal;
const targets = [...st.map((s) => [s.site, undefined, `f${s.site}`]), ...(cfg?.learners ?? []).map((l, i) => [l.site, l.do_name, `l${i}`])];
const pct = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const at = (p) => (s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null);
  return { n: s.length, p50: at(0.5), p99: at(0.99), max: s.length ? s[s.length - 1] : null };
};
for (const [site, name, tag] of targets) {
  try {
    const v = (await member(chain, site, "/stats", undefined, 30000, name)).v ?? {};
    const tr = v.trace ?? [];
    const sent = new Map();
    const rtt = [], apply = [], pending = [];
    let unanswered = 0;
    let applied = 0;
    for (const [at, kind, , batch, index, ms] of tr) {
      if (kind === 1) sent.set(batch, at);
      else if (kind === 3 && ms === 0 && sent.has(batch)) {
        rtt.push(at - sent.get(batch));
        sent.delete(batch);
        if (index && index <= applied) apply.push(0);
        else if (index) pending.push([index, at]);
      } else if (kind === 4) {
        applied = index;
        while (pending.length && pending[0][0] <= index) apply.push(at - pending.shift()[1]);
      }
    }
    unanswered = sent.size;
    console.log(tag, JSON.stringify({ role: v.role, entries: tr.length, rtt: pct(rtt), apply: pct(apply), unanswered }));
  } catch (e) {
    console.log(tag, String(e).slice(0, 80));
  }
}
